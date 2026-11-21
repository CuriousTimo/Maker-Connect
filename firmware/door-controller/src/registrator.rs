//! Port of `registrator.cpp` / `registrator.hpp`: registration by code over HTTP.
//! `POST /p/register`, then `GET /p/register` with the header `Code` every 5 s until `200` with `Token`.

use crate::DEVICE_TYPE;
use crate::device::{CONNECT_TOKEN_LEN, Printer};
use crate::status::*;
use crate::traits::{HttpRequest, HttpResponse, Method, Net};

/// registrator.cpp:41
pub const REGISTER_URL: &str = "/p/register";
/// registrator.cpp:44
pub const SLEEP_MAX_MS: u32 = 125;
/// registrator.cpp:46
pub const POLL_INTERVAL_MS: u32 = 5 * 1000;
/// registrator.hpp:14: `CODE_SIZE`
pub const CODE_SIZE: usize = 25;
/// registrator.hpp:45
pub const STARTING_RETRIES: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Init,
    GotCode,
    Done,
    Error,
}

pub struct Registrator {
    code: String,
    status: Status,
    last_comm: u32,
    retries_left: u8,
}

/// Header value by name, as `ExtractCode`/`ExtractToken` collect it (`registrator.cpp:117-137`): the
/// value is cut at `max` characters (the buffer minus the `\0`).
fn extract_header(resp: &HttpResponse, name: &str, max: usize) -> String {
    resp.headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.chars().take(max).collect())
        .unwrap_or_default()
}

impl Default for Registrator {
    fn default() -> Self {
        Registrator {
            code: String::new(),
            status: Status::Init,
            last_comm: 0,
            retries_left: STARTING_RETRIES,
        }
    }
}

impl Registrator {
    pub fn new() -> Registrator {
        Registrator::default()
    }

    /// registrator.cpp:242-244
    pub fn get_code(&self) -> &str {
        &self.code
    }

    /// registrator.cpp:52-72 `PostRenderer`: the body, in Buddy's field order. `printer_type` is Part 1's
    /// string where Buddy writes `a.b.c` numbers.
    fn post_body(printer: &Printer) -> Vec<u8> {
        let info = printer.printer_info();
        let j = |s: &str| serde_json::to_string(s).expect("a string always serialises");
        format!(
            "{{\"sn\":{},\"fingerprint\":{},\"printer_type\":{},\"firmware\":{}}}",
            j(&info.serial_number),
            j(&info.fingerprint),
            j(DEVICE_TYPE),
            j(&info.firmware_version)
        )
        .into_bytes()
    }

    /// registrator.cpp:234-240 `bail`
    fn bail(&mut self, net: &mut dyn Net, error: OnlineError, retries: Option<u8>) -> CommResult {
        if retries == Some(0) {
            net.invalidate();
            self.status = Status::Error;
        }
        CommResult::ErrWithRetry(ErrWithRetry {
            err: error,
            retry: retries,
        })
    }

    /// registrator.cpp:140-232 `Registrator::communicate`
    pub fn communicate(&mut self, printer: &mut Printer, net: &mut dyn Net) -> CommResult {
        let (_config, cfg_changed) = printer.config(true);
        if cfg_changed {
            net.invalidate();
            return CommResult::Nothing;
        }

        let n = printer.now();
        let since_last = n.wrapping_sub(self.last_comm);
        if since_last <= POLL_INTERVAL_MS {
            let sleep_for = (POLL_INTERVAL_MS - since_last).min(SLEEP_MAX_MS);
            printer.sleep(sleep_for);
            return CommResult::Nothing;
        }

        match self.status {
            Status::Init => {
                let req = HttpRequest {
                    method: Method::Post,
                    path: REGISTER_URL,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: Self::post_body(printer),
                };
                let result = net.http(&req);
                self.last_comm = printer.now();
                let resp = match result {
                    Err(e) => {
                        self.retries_left = self.retries_left.saturating_sub(1);
                        let retries = Some(self.retries_left);
                        return self.bail(net, err_to_status(e), retries);
                    }
                    Ok(r) => r,
                };
                // :171: Connect's bodies are not read, so the connection is thrown out.
                net.invalidate();
                let code = extract_header(&resp, "Code", CODE_SIZE);
                if resp.status == 200 && !code.is_empty() {
                    self.code = code;
                    self.status = Status::GotCode;
                    CommResult::Status(ConnectionStatus::RegistrationCode)
                } else {
                    self.retries_left = self.retries_left.saturating_sub(1);
                    let retries = Some(self.retries_left);
                    self.bail(net, OnlineError::Server, retries)
                }
            }
            Status::GotCode => {
                let req = HttpRequest {
                    method: Method::Get,
                    path: REGISTER_URL,
                    headers: vec![("Code".into(), self.code.clone())],
                    body: vec![],
                };
                let result = net.http(&req);
                self.last_comm = printer.now();
                let resp = match result {
                    Err(e) => return self.bail(net, err_to_status(e), None),
                    Ok(r) => r,
                };
                net.invalidate();
                match resp.status {
                    // :210-214: 202 is what the docs say, 204 what would make sense.
                    202 | 204 => CommResult::Nothing,
                    200 => {
                        let token = extract_header(&resp, "Token", CONNECT_TOKEN_LEN);
                        if token.is_empty() {
                            return self.bail(net, OnlineError::Server, None);
                        }
                        printer.init_connect(&token);
                        self.status = Status::Done;
                        CommResult::Status(ConnectionStatus::RegistrationDone)
                    }
                    _ => self.bail(net, OnlineError::Server, None),
                }
            }
            Status::Done | Status::Error => CommResult::Nothing,
        }
    }
}
