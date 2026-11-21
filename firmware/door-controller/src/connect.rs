//! Port of `connect.cpp` (WebSocket mode only, `#if WEBSOCKET()`) and of the sleep of `sleep.cpp`.

use crate::command::{Command, CommandData, CommandId, parse_json_command};
use crate::device::{FINGERPRINT_HDR_SIZE, Printer};
use crate::planner::{Action, ActionResult, Planner};
use crate::registrator::Registrator;
use crate::render::{render_event, render_telemetry};
use crate::status::*;
use crate::traits::{Net, WsMessage, WsSend, WsUpgrade};

/// connect.cpp:50: send a ping when we were silent this long.
pub const PING_INACTIVITY_MS: u32 = 15_000;
/// connect.cpp:198 `MAX_RESP_SIZE`
pub const MAX_RESP_SIZE: usize = 512;
/// connect.cpp:365 `HDR_LEN`: the type and the command id as 8 hex digits.
pub const HDR_LEN: usize = 9;
/// connect.cpp:204 `CONFIG_ENABLED_RECHECK`
pub const CONFIG_ENABLED_RECHECK: u32 = 1000;
/// sleep.cpp:26 `IDLE_WAIT`
pub const IDLE_WAIT: u32 = 100;
/// connect.cpp:389: a ping's payload may not be fragmented.
const MAX_PING_LEN: usize = 126;

pub struct Client {
    planner: Planner,
    registrator: Option<Registrator>,
    registration: bool,
    websocket: bool,
    last_send: u32,
    pub last_known_status: ConnectionStatus,
    pub last_connection_error: OnlineError,
    pub retries_left: Option<u8>,
    registration_code: Option<String>,
}

impl Client {
    /// connect.cpp:909-912
    pub fn new(printer: &mut Printer) -> Client {
        Client {
            planner: Planner::new(printer),
            registrator: None,
            registration: false,
            websocket: false,
            last_send: 0,
            last_known_status: ConnectionStatus::Unknown,
            last_connection_error: OnlineError::NoError,
            retries_left: None,
            registration_code: None,
        }
    }

    /// connect.cpp:930-935 `request_registration`
    pub fn request_registration(&mut self) {
        self.registration = true;
    }

    /// connect.cpp:937-942 `leave_registration`
    pub fn leave_registration(&mut self) {
        self.registration = false;
    }

    /// connect.cpp:944-953 `registration_code`
    pub fn registration_code(&self) -> Option<&str> {
        match self.last_known_status {
            ConnectionStatus::RegistrationCode | ConnectionStatus::RegistrationDone => {
                self.registration_code.as_deref()
            }
            _ => None,
        }
    }

    pub fn websocket_open(&self) -> bool {
        self.websocket
    }

    /// connect.cpp:63-98 `process_status`
    fn process_status(&mut self, status: CommResult, err_status: ConnectionStatus) {
        match status {
            CommResult::Nothing => {}
            CommResult::Error(e) => {
                self.last_known_status = err_status;
                self.last_connection_error = e;
            }
            CommResult::Status(s) => {
                self.last_known_status = s;
                match s {
                    ConnectionStatus::Ok
                    | ConnectionStatus::NoConfig
                    | ConnectionStatus::Off
                    | ConnectionStatus::RegistrationCode
                    | ConnectionStatus::RegistrationDone => {
                        self.last_connection_error = OnlineError::NoError;
                        self.retries_left = None;
                    }
                    _ => {}
                }
            }
            CommResult::ErrWithRetry(e) => {
                self.last_connection_error = e.err;
                self.retries_left = e.retry;
                if e.retry == Some(0) {
                    self.last_known_status = err_status;
                }
            }
        }
    }

    /// Throw the connection and the socket away (`conn_factory.invalidate()`, `websocket.reset()`).
    fn drop_connection(&mut self, net: &mut dyn Net) {
        net.invalidate();
        self.websocket = false;
    }

    /// connect.cpp:872-902 `Connect::run`, one iteration.
    pub fn run_once(&mut self, printer: &mut Printer, net: &mut dyn Net) {
        let reg_running = self.registrator.is_some();
        if self.registration && reg_running {
            let reg = self.registrator.as_mut().expect("checked above");
            let new_status = reg.communicate(printer, net);
            self.registration_code = Some(reg.get_code().to_string());
            self.process_status(new_status, ConnectionStatus::RegistrationError);
        } else if self.registration && !reg_running {
            self.registrator = Some(Registrator::new());
            self.last_known_status = ConnectionStatus::Unknown;
            self.last_connection_error = OnlineError::NoError;
            self.retries_left = None;
            self.drop_connection(net);
        } else if !self.registration && reg_running {
            self.last_known_status = ConnectionStatus::Unknown;
            self.last_connection_error = OnlineError::NoError;
            self.retries_left = None;
            self.registration_code = None;
            self.registrator = None;
            self.planner = Planner::new(printer);
            self.drop_connection(net);
        } else {
            let new_status = self.communicate(printer, net);
            self.process_status(new_status, ConnectionStatus::Error);
        }
    }

    /// connect.cpp:787-870 `Connect::communicate`.
    pub fn communicate(&mut self, printer: &mut Printer, net: &mut dyn Net) -> CommResult {
        let (config, cfg_changed) = printer.config(true);

        if cfg_changed {
            self.drop_connection(net);
            self.planner.reset();
        }

        if config.token.is_empty() {
            self.planner.reset();
            printer.sleep(CONFIG_ENABLED_RECHECK);
            return CommResult::Status(ConnectionStatus::NoConfig);
        }

        let mut wake_on_readable = false;
        if self.websocket {
            wake_on_readable = true;
            if printer.now().wrapping_sub(self.last_send) > PING_INACTIVITY_MS {
                return self.send_ping(printer, net);
            }
        } else {
            // :837 the server node might want fresh telemetry.
            self.planner.reset_telemetry();
        }

        let action = self.planner.next_action(printer, wake_on_readable);

        // :843-846 sleeping does not need the connection.
        if let Action::Sleep {
            ms,
            run_background,
            wake_on_readable,
        } = action
        {
            self.perform_sleep(printer, net, ms, run_background, wake_on_readable);
            return CommResult::Nothing;
        }

        let prepared = self.prepare_connection(printer, net, &config.token);
        if prepared != CommResult::Nothing {
            if matches!(action, Action::Event(_) | Action::SendTelemetry(_)) {
                self.planner.action_done(printer, ActionResult::Failed);
            }
            return prepared;
        }

        match action {
            Action::ReadCommand => self.receive_command(printer, net),
            other => self.send_command(printer, net, other),
        }
    }

    /// connect.cpp:265-276 `send_ping`
    fn send_ping(&mut self, printer: &mut Printer, net: &mut dyn Net) -> CommResult {
        if let Err(e) = net.ws_send(WsSend::Ping, &[]) {
            self.drop_connection(net);
            return CommResult::Error(err_to_status(e));
        }
        self.last_send = printer.now();
        CommResult::Status(ConnectionStatus::Ok)
    }

    /// connect.cpp:559-637 `prepare_connection`. The headers are Buddy's `UpgradeRequest`
    /// (`:160-171`): `Fingerprint` (cut to 16 characters), `Token`, `Upgrade`, `Sec-WebSocket-Key`,
    /// `Sec-WebSocket-Version`, `Sec-WebSocket-Protocol`; the key and `Upgrade` belong to the `Net` binding.
    fn prepare_connection(
        &mut self,
        printer: &mut Printer,
        net: &mut dyn Net,
        token: &str,
    ) -> CommResult {
        if !self.websocket {
            self.last_known_status = ConnectionStatus::Connecting;
            let fingerprint: String = printer
                .printer_info()
                .fingerprint
                .chars()
                .take(FINGERPRINT_HDR_SIZE)
                .collect();
            let headers = vec![
                ("Fingerprint".to_string(), fingerprint),
                ("Token".to_string(), token.to_string()),
                (
                    "Sec-WebSocket-Protocol".to_string(),
                    "prusa-connect".to_string(),
                ),
            ];
            match net.ws_upgrade(&headers) {
                Err(e) => {
                    self.drop_connection(net);
                    return CommResult::Error(err_to_status(e));
                }
                Ok(WsUpgrade::Switched) => {
                    self.websocket = true;
                    self.last_send = printer.now();
                }
                Ok(WsUpgrade::Unverified) => {
                    self.drop_connection(net);
                    return CommResult::Error(OnlineError::Server);
                }
                Ok(WsUpgrade::Refused(status)) => {
                    self.drop_connection(net);
                    self.planner.action_done(printer, ActionResult::Refused);
                    return CommResult::Error(match status {
                        400 => OnlineError::Internal,
                        401 | 403 => OnlineError::Auth,
                        _ => OnlineError::Server,
                    });
                }
            }
        }
        CommResult::Nothing
    }

    /// connect.cpp:639-680 `send_command`: one text message, then `action_done(Ok)`.
    fn send_command(
        &mut self,
        printer: &mut Printer,
        net: &mut dyn Net,
        action: Action,
    ) -> CommResult {
        if !self.websocket {
            self.planner.action_done(printer, ActionResult::Failed);
            return CommResult::Error(OnlineError::Network);
        }
        let bg = self.planner.background_command_id();
        let params = printer.params(self.planner.busy());
        let body = match action {
            Action::SendTelemetry(_) => render_telemetry(&params, bg),
            Action::Event(ev) => {
                render_event(&ev, &params, printer.printer_info(), printer.lock_count())
            }
            _ => return CommResult::Error(OnlineError::Internal),
        };
        if let Err(e) = net.ws_send(WsSend::Text, body.as_bytes()) {
            self.drop_connection(net);
            self.planner.action_done(printer, ActionResult::Failed);
            return CommResult::Error(err_to_status(e));
        }
        self.last_send = printer.now();
        self.planner.action_done(printer, ActionResult::Ok);
        CommResult::Status(ConnectionStatus::Ok)
    }

    /// connect.cpp:357-557 `receive_command`. Messages arrive whole (the `Net` binding joins
    /// fragments), so Buddy's "Oversized command" (`:308-314`) is a message longer than its buffer.
    fn receive_command(&mut self, printer: &mut Printer, net: &mut dyn Net) -> CommResult {
        loop {
            // :370 `first` stays true after a control message, so it is always `Some(0)` here.
            let res = net.ws_receive(Some(0));
            let msg = match res {
                Ok(None) => return CommResult::Status(ConnectionStatus::Ok),
                Err(e) => {
                    self.drop_connection(net);
                    return CommResult::Error(err_to_status(e));
                }
                Ok(Some(m)) => m,
            };
            match msg {
                WsMessage::Ping(data) => {
                    if data.len() > MAX_PING_LEN {
                        self.drop_connection(net);
                        return CommResult::Error(OnlineError::Confused);
                    }
                    if let Err(e) = net.ws_send(WsSend::Pong, &data) {
                        self.drop_connection(net);
                        return CommResult::Error(err_to_status(e));
                    }
                    self.last_send = printer.now();
                    continue;
                }
                WsMessage::Pong => continue,
                WsMessage::Close => {
                    self.drop_connection(net);
                    return CommResult::Error(OnlineError::Network);
                }
                WsMessage::Data(buf) => {
                    return self.handle_message(printer, buf);
                }
            }
        }
    }

    /// connect.cpp:445-530: the header, the type letter and the handler.
    fn handle_message(&mut self, printer: &mut Printer, buf: Vec<u8>) -> CommResult {
        if buf.len() < HDR_LEN {
            self.planner.command(
                printer,
                Command {
                    id: 0,
                    data: CommandData::Broken("Message too short to contain header"),
                },
            );
            return CommResult::Status(ConnectionStatus::Ok);
        }
        let id = std::str::from_utf8(&buf[1..9])
            .ok()
            .and_then(|s| CommandId::from_str_radix(s, 16).ok());
        let Some(id) = id else {
            self.planner.command(
                printer,
                Command {
                    id: 0,
                    data: CommandData::Broken("Could not parse command ID"),
                },
            );
            return CommResult::Status(ConnectionStatus::Ok);
        };
        let body = &buf[HDR_LEN..];
        match buf[0] {
            // :469-477: one command at a time in the buffer.
            // (Our background command holds no buffer, so Planner::command answers it, :1085-1093.)
            b'J' => {
                if body.len() > MAX_RESP_SIZE {
                    // :308-314
                    self.planner.command(
                        printer,
                        Command {
                            id,
                            data: CommandData::Broken("Oversized command"),
                        },
                    );
                } else {
                    self.planner.command(printer, parse_json_command(id, body));
                }
            }
            // :510-514 a debug message is thrown away.
            b'D' => {}
            // :518-523. `G`, `F` (G-code) and `T` (file chunks) are not ported (open question).
            _ => self.planner.command(
                printer,
                Command {
                    id,
                    data: CommandData::Broken("Unrecognized type of message"),
                },
            ),
        }
        CommResult::Status(ConnectionStatus::Ok)
    }

    /// sleep.cpp:264-362 `Sleep::perform`, for the tasks this board has: the background command
    /// (`SubmitBackgroundCmd`, `:162-194`) and the connection to watch (`ReadableConnection`, `:231-248`).
    fn perform_sleep(
        &mut self,
        printer: &mut Printer,
        net: &mut dyn Net,
        ms: u32,
        run_background: bool,
        wake_on_readable: bool,
    ) {
        let mut remaining = ms;
        let mut need_run = run_background || wake_on_readable;
        while remaining > 0 || need_run {
            need_run = false;
            if run_background && self.planner.run_background(printer) {
                // :183-185 something changed, let the outer loop deal with it.
                return;
            }
            if wake_on_readable && net.ws_readable(0) {
                self.planner.notify_command_waiting();
                return;
            }
            let left = remaining.min(IDLE_WAIT);
            if left == 0 {
                break;
            }
            // :349-351 sleep between two iterations; with a socket to watch, wait on it.
            if wake_on_readable {
                if net.ws_readable(left) {
                    self.planner.notify_command_waiting();
                    return;
                }
            } else {
                printer.sleep(left);
            }
            remaining -= left;
        }
    }
}
