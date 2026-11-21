//! WebSocket transport for Connect SDK — telemetry and events (`ws.py`).
//!
//! `ws.py` runs `websocket.WebSocketApp.run_forever(ping_interval=30, ping_timeout=10)` in a
//! daemon thread. This port runs the same loop in a thread with `tungstenite`: it connects,
//! answers server pings, sends an empty ping every 30 s and closes when no pong came within
//! 10 s of one, as `run_forever` does, and does not reconnect by itself (`run_forever`'s
//! `reconnect` is 0), so `Printer.ensure_ws_connected` reconnects on the next send.
//!
//! The loop waits for data as websocket-client does (`select` on the socket, no lock held): on a
//! clone of the TCP socket, with a short timeout that only paces the loop and never reaches the
//! wire. Only when data has come does it take the socket's lock to read, so a `send` from another
//! thread is never kept waiting by a reader holding the lock.
//!
//! websocket-client is the SDK's dependency (`requirements.txt`: `websocket-client>=1.6.0`), not
//! part of the reference commit. What this module takes from it, all to be checked by running
//! websocket-client in step A4:
//! - the `Origin` header (`suppress_origin` False);
//! - the ping thread and `check()`'s "ping/pong timed out" rule;
//! - `close()` sending status 1000 and waiting up to 3 s for the server's close frame;
//! - a callback's exception going to `on_error`, the connection kept.
//!
//! Known to differ (not covered): the handshake's header order (tungstenite writes an
//! `http::HeaderMap`, which keeps no insertion order) and the `Host` header for a URL
//! that names the default port are tungstenite's; a header value that is not valid HTTP fails
//! the connect, where websocket-client writes the raw line; proxies from the environment and
//! websocket-client's CA bundle choice are not read (tungstenite verifies against webpki-roots,
//! the Mozilla bundle).

use std::net::TcpStream;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::{HeaderName, HeaderValue};
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::clock;
use crate::consts;
use crate::util::py_json_dumps;

/// `run_forever(ping_interval=30, ...)`
pub const PING_INTERVAL: f64 = 30.0;
/// `run_forever(..., ping_timeout=10)`
pub const PING_TIMEOUT: f64 = 10.0;
/// `WebSocket.close(timeout=3)`: how long a close waits for the server's close frame.
const CLOSE_TIMEOUT: f64 = 3.0;
/// How long the loop waits for data before it looks at its pings again.
const POLL: Duration = Duration::from_millis(10);

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;
pub type MessageCallback = Arc<dyn Fn(String) + Send + Sync>;

struct Shared {
    socket: Mutex<Option<Socket>>,
    connected: AtomicBool,
    keep_running: AtomicBool,
    /// `_open_event`
    opened: Mutex<bool>,
    opened_cv: Condvar,
}

impl Shared {
    fn socket(&self) -> MutexGuard<'_, Option<Socket>> {
        self.socket.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_open_event(&self) {
        *self.opened.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.opened_cv.notify_all();
    }
}

struct Running {
    shared: Arc<Shared>,
    done: mpsc::Receiver<()>,
}

/// Persistent WebSocket connection to Connect.
///
/// Runs the connection loop in a background thread so that `Printer.loop` can call `send()`
/// without blocking. Incoming frames are forwarded to the `on_message` callback supplied at
/// construction (`Printer.handle_ws_message`).
pub struct PrinterWs {
    on_message: MessageCallback,
    running: Mutex<Option<Running>>,
    /// `const.CONNECTION_TIMEOUT`, as `connect` and `disconnect` read it.
    pub connection_timeout: Duration,
}

impl PrinterWs {
    pub fn new(on_message: MessageCallback) -> PrinterWs {
        PrinterWs {
            on_message,
            running: Mutex::new(None),
            connection_timeout: Duration::from_secs(consts::CONNECTION_TIMEOUT),
        }
    }

    fn running(&self) -> MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// True when the WebSocket handshake completed and socket is open.
    pub fn connected(&self) -> bool {
        self.running()
            .as_ref()
            .is_some_and(|r| r.shared.connected.load(Ordering::SeqCst))
    }

    /// (Re)connect to `url` using `headers` for the WS handshake; a header whose value is None
    /// is left out.
    ///
    /// Blocks until the connection opens, fails, or CONNECTION_TIMEOUT elapses. Safe to call
    /// when already connected — disconnects first.
    pub fn connect(&self, url: &str, headers: &[(String, Option<String>)]) {
        self.disconnect();

        let shared = Arc::new(Shared {
            socket: Mutex::new(None),
            connected: AtomicBool::new(false),
            keep_running: AtomicBool::new(true),
            opened: Mutex::new(false),
            opened_cv: Condvar::new(),
        });
        let header_list: Vec<(String, String)> = headers
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.clone()?)))
            .collect();
        let (done_tx, done) = mpsc::channel();
        let thread_shared = shared.clone();
        let on_message = self.on_message.clone();
        let url_owned = url.to_owned();
        thread::spawn(move || {
            run_forever(&thread_shared, &url_owned, &header_list, &on_message);
            thread_shared.connected.store(false, Ordering::SeqCst);
            *thread_shared.socket() = None;
            let _ = done_tx.send(());
        });
        *self.running() = Some(Running {
            shared: shared.clone(),
            done,
        });

        let timeout = self.connection_timeout;
        let opened = shared.opened.lock().unwrap_or_else(|p| p.into_inner());
        let (opened, _) = shared
            .opened_cv
            .wait_timeout_while(opened, timeout, |opened| !*opened)
            .unwrap_or_else(|p| p.into_inner());
        if !*opened {
            log::warn!("WS connect timed out: {url}");
        }
    }

    /// Close the WebSocket and wait for the background thread to stop.
    pub fn disconnect(&self) {
        let Some(running) = self.running().take() else {
            return;
        };
        running.shared.keep_running.store(false, Ordering::SeqCst);
        let _ = running.done.recv_timeout(self.connection_timeout);
    }

    /// Serialize `payload` to JSON and send as a text frame.
    ///
    /// Returns true on success, false when not connected or on any error. Thread-safe (one send
    /// at a time via lock).
    pub fn send(&self, payload: &Value) -> bool {
        let shared = match self.running().as_ref() {
            Some(running) if running.shared.connected.load(Ordering::SeqCst) => {
                running.shared.clone()
            }
            _ => {
                log::warn!("WS not connected, dropping: {payload}");
                return false;
            }
        };
        let mut socket = shared.socket();
        let Some(socket) = socket.as_mut() else {
            log::warn!("WS not connected, dropping: {payload}");
            return false;
        };
        match socket.send(Message::Text(py_json_dumps(payload).into())) {
            Ok(()) => true,
            Err(err) => {
                log::error!("WS send failed: {err}");
                false
            }
        }
    }
}

impl Drop for PrinterWs {
    fn drop(&mut self) {
        if let Some(running) = self.running().take() {
            running.shared.keep_running.store(false, Ordering::SeqCst);
        }
    }
}

/// websocket-client's handshake adds `Origin: http(s)://host[:port]` (`suppress_origin` is
/// False in `run_forever`).
fn origin(request: &tungstenite::handshake::client::Request) -> Option<String> {
    let uri = request.uri();
    let scheme = if uri.scheme_str() == Some("wss") {
        "https"
    } else {
        "http"
    };
    let host = uri.host()?;
    let default_port = if scheme == "https" { 443 } else { 80 };
    match uri.port_u16() {
        Some(port) if port != default_port => Some(format!("{scheme}://{host}:{port}")),
        _ => Some(format!("{scheme}://{host}")),
    }
}

fn tcp(socket: &Socket) -> Option<&TcpStream> {
    match socket.get_ref() {
        MaybeTlsStream::Plain(tcp) => Some(tcp),
        MaybeTlsStream::Rustls(tls) => Some(&tls.sock),
        _ => None,
    }
}

fn set_read_timeout(socket: &Socket, timeout: Duration) {
    if let Some(tcp) = tcp(socket) {
        let _ = tcp.set_read_timeout(Some(timeout));
    }
}

/// Whether data (or the end of the stream) is there to read, waiting up to the socket's read
/// timeout, without the socket's lock (websocket-client's `select`).
fn readable(waiter: &TcpStream) -> bool {
    let mut byte = [0u8; 1];
    match waiter.peek(&mut byte) {
        Ok(_) => true,
        Err(err) => !matches!(
            err.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
    }
}

fn is_timeout(err: &tungstenite::Error) -> bool {
    matches!(err, tungstenite::Error::Io(io)
        if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// `_handle_error`: log, and unblock `connect()` if still waiting.
fn handle_error(shared: &Shared, error: &dyn std::fmt::Display) {
    log::error!("WS error: {error}");
    shared.set_open_event();
}

fn run_forever(
    shared: &Shared,
    url: &str,
    headers: &[(String, String)],
    on_message: &MessageCallback,
) {
    let mut request = match url.into_client_request() {
        Ok(request) => request,
        Err(err) => return handle_error(shared, &err),
    };
    let mut extra = Vec::new();
    if let Some(origin) = origin(&request) {
        extra.push(("Origin".to_owned(), origin));
    }
    extra.extend_from_slice(headers);
    for (key, value) in extra {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(&value),
        ) else {
            return handle_error(shared, &format!("invalid header {key}"));
        };
        request.headers_mut().append(name, value);
    }

    let socket = match tungstenite::connect(request) {
        Ok((socket, _response)) => socket,
        Err(err) => return handle_error(shared, &err),
    };
    set_read_timeout(&socket, POLL);
    // The loop waits for data on its own handle of the socket (`select`), not under the lock.
    let waiter = match tcp(&socket).map(TcpStream::try_clone) {
        Some(Ok(waiter)) => waiter,
        Some(Err(err)) => return handle_error(shared, &err),
        None => return handle_error(shared, &"unknown stream"),
    };
    *shared.socket() = Some(socket);
    shared.connected.store(true, Ordering::SeqCst);
    // _handle_open
    log::debug!("WS connection opened");
    shared.set_open_event();

    let opened_at = clock::monotonic();
    let mut last_ping_tm = 0.0;
    let mut last_pong_tm = 0.0;
    let mut closing_since: Option<f64> = None;
    // A message was read: more may already be in the reader's buffer, read on before waiting.
    let mut more = false;
    loop {
        let now = clock::monotonic();
        if closing_since.is_none() && !shared.keep_running.load(Ordering::SeqCst) {
            // WebSocketApp.close(): sock.close(status=STATUS_NORMAL, reason=b"")
            shared.connected.store(false, Ordering::SeqCst);
            if let Some(socket) = shared.socket().as_mut() {
                let frame = CloseFrame {
                    code: CloseCode::Normal,
                    reason: "".into(),
                };
                let _ = socket.close(Some(frame));
            }
            closing_since = Some(now);
        }
        if closing_since.is_some_and(|since| now - since > CLOSE_TIMEOUT) {
            break;
        }

        if closing_since.is_none() {
            // _send_ping: every ping_interval, with an empty payload
            let since_ping = if last_ping_tm == 0.0 {
                now - opened_at
            } else {
                now - last_ping_tm
            };
            if since_ping >= PING_INTERVAL {
                last_ping_tm = now;
                if let Some(socket) = shared.socket().as_mut()
                    && let Err(err) = socket.send(Message::Ping(Default::default()))
                {
                    handle_error(shared, &err);
                    break;
                }
            }
            // check(): "ping/pong timed out"
            let has_timeout_expired = now - last_ping_tm > PING_TIMEOUT;
            let pong_not_arrived = last_pong_tm - last_ping_tm < 0.0;
            let pong_too_late = last_pong_tm - last_ping_tm > PING_TIMEOUT;
            if last_ping_tm != 0.0 && has_timeout_expired && (pong_not_arrived || pong_too_late) {
                handle_error(shared, &"ping/pong timed out");
                shared.keep_running.store(false, Ordering::SeqCst);
                continue;
            }
        }

        if !more && !readable(&waiter) {
            continue;
        }
        let read = match shared.socket().as_mut() {
            Some(socket) => socket.read(),
            None => break,
        };
        more = read.is_ok();
        match read {
            Ok(Message::Text(text)) => {
                let text = text.to_string();
                // websocket-client catches an exception in a callback and reports it to on_error
                if catch_unwind(AssertUnwindSafe(|| on_message(text))).is_err() {
                    handle_error(shared, &"on_message callback panicked");
                }
            }
            Ok(Message::Pong(_)) => last_pong_tm = clock::monotonic(),
            Ok(Message::Close(frame)) => {
                // _handle_close
                log::debug!("WS closed: {frame:?}");
                shared.connected.store(false, Ordering::SeqCst);
                closing_since.get_or_insert(now);
            }
            Ok(_) => {}
            Err(err) if is_timeout(&err) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => break,
            Err(err) => {
                if closing_since.is_none() {
                    handle_error(shared, &err);
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// The handshake headers, a sender of frames to the client, and the client's frames.
    type Accepted = (
        Vec<(String, String)>,
        mpsc::Sender<String>,
        mpsc::Receiver<String>,
    );

    /// A WebSocket server on a free port, run by tungstenite's server side; one thread per
    /// connection. Sending `<drop>` to a connection drops its socket without a close frame.
    #[allow(clippy::result_large_err)] // tungstenite's handshake callback signature
    fn server() -> (String, mpsc::Receiver<Accepted>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/p/ws", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let tx = tx.clone();
                thread::spawn(move || serve(stream, &tx));
            }
        });
        (url, rx)
    }

    #[allow(clippy::result_large_err)]
    fn serve(stream: TcpStream, tx: &mpsc::Sender<Accepted>) {
        let headers = Arc::new(Mutex::new(Vec::new()));
        let seen = headers.clone();
        let callback = move |req: &tungstenite::handshake::server::Request, res| {
            for (name, value) in req.headers() {
                seen.lock()
                    .unwrap()
                    .push((name.to_string(), value.to_str().unwrap().to_owned()));
            }
            Ok(res)
        };
        let Ok(mut ws) = tungstenite::accept_hdr(stream, callback) else {
            return;
        };
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let (to_client, outgoing) = mpsc::channel::<String>();
        let (incoming_tx, incoming) = mpsc::channel::<String>();
        let _ = tx.send((headers.lock().unwrap().clone(), to_client, incoming));
        loop {
            if let Ok(text) = outgoing.try_recv() {
                if text == "<drop>" {
                    return;
                }
                ws.send(Message::Text(text.into())).unwrap();
            }
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let _ = incoming_tx.send(text.to_string());
                }
                Ok(Message::Close(_)) => {
                    let _ = incoming_tx.send("<close>".into());
                }
                Ok(_) => {}
                Err(err) if is_timeout(&err) => {}
                Err(_) => return,
            }
        }
    }

    /// Wait until `f` holds, for at most 5 s.
    pub(super) fn eventually(f: impl Fn() -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Ours: a `send` from another thread is not kept waiting by the reading loop (it waits for
    /// data without the socket's lock), with the server quiet: every send is quick, none waits
    /// on the loop.
    #[test]
    fn a_send_is_not_kept_waiting_by_the_reader() {
        let (url, accepted) = server();
        let sock = PrinterWs::new(Arc::new(|_| {}));
        sock.connect(&url, &[]);
        let (_, _to_client, sent) = accepted.recv().unwrap();
        let mut slowest = Duration::ZERO;
        for i in 0..200 {
            let start = std::time::Instant::now();
            assert!(sock.send(&serde_json::json!({ "i": i })));
            slowest = slowest.max(start.elapsed());
        }
        for _ in 0..200 {
            sent.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(
            slowest < Duration::from_millis(5),
            "a send waited {slowest:?}"
        );
        sock.disconnect();
    }

    /// `tests/test_ws.py` at `c0d7d233`. The SDK replaces `WebSocketApp` with a fake; these run
    /// against a real server instead, so each checks what its Python test checks, on the wire.
    /// `test_connected_without_socket` has no counterpart: here the socket exists only once the
    /// handshake is done, which `test_connect_timeout` covers.
    mod sdk_tests {
        use super::*;
        use serde_json::json;

        const URL_PATH: &str = "/p/ws";

        fn quiet() -> PrinterWs {
            PrinterWs::new(Arc::new(|_| {}))
        }

        #[test]
        fn test_not_connected() {
            // Nothing is sent without a connection
            let sock = quiet();
            assert!(!sock.connected());
            assert!(!sock.send(&json!({"a": 1})));
            sock.disconnect(); // harmless
        }

        #[test]
        fn test_connect_and_send() {
            // Headers are passed to the handshake and payload is sent as JSON
            let (url, accepted) = server();
            assert!(url.ends_with(URL_PATH));
            let sock = quiet();
            sock.connect(
                &url,
                &[
                    ("Token".into(), Some("t".into())),
                    ("Skipped".into(), None),
                    ("Fingerprint".into(), Some("f".into())),
                ],
            );
            let (headers, to_client, sent) = accepted.recv().unwrap();
            // As a set: the order on the wire is tungstenite's (see the module doc).
            let mine: std::collections::BTreeSet<(String, String)> = headers
                .iter()
                .filter(|(n, _)| ["token", "skipped", "fingerprint"].contains(&n.as_str()))
                .cloned()
                .collect();
            let want =
                [("token", "t"), ("fingerprint", "f")].map(|(n, v)| (n.to_owned(), v.to_owned()));
            assert_eq!(mine, want.into_iter().collect());
            assert_eq!((PING_INTERVAL, PING_TIMEOUT), (30.0, 10.0));
            assert!(sock.connected());
            assert!(sock.send(&json!({"a": 1})));
            let got: Value = serde_json::from_str(&sent.recv().unwrap()).unwrap();
            assert_eq!(got, json!({"a": 1}));

            to_client.send("<drop>".into()).unwrap();
            assert!(eventually(|| !sock.connected()));
            assert!(!sock.send(&json!({"a": 2})));
            assert!(sent.recv_timeout(Duration::from_millis(100)).is_err());
        }

        #[test]
        fn test_reconnect_closes_previous() {
            // Connecting again closes the old connection first
            let (url, accepted) = server();
            let sock = quiet();
            sock.connect(&url, &[]);
            let (_, _, first) = accepted.recv().unwrap();
            sock.connect(&url, &[]);
            let (_, _, second) = accepted.recv().unwrap();
            assert_eq!(
                first.recv_timeout(Duration::from_secs(5)).unwrap(),
                "<close>"
            );
            assert!(second.try_recv().is_err());
            sock.disconnect();
            assert_eq!(
                second.recv_timeout(Duration::from_secs(5)).unwrap(),
                "<close>"
            );
            assert!(!sock.connected());
        }

        #[test]
        fn test_send_failure() {
            // An exception while sending is reported as failure: the peer is gone
            let (url, accepted) = server();
            let sock = quiet();
            sock.connect(&url, &[]);
            let (_, to_client, _) = accepted.recv().unwrap();
            to_client.send("<drop>".into()).unwrap();
            assert!(eventually(|| !sock.send(&json!({"a": 1}))));
        }

        #[test]
        fn test_connect_timeout() {
            // connect() does not block forever when the handshake never ends
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("ws://{}{URL_PATH}", listener.local_addr().unwrap());
            let mut sock = quiet();
            sock.connection_timeout = Duration::from_millis(10);
            let started = std::time::Instant::now();
            sock.connect(&url, &[]);
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(!sock.connected());
            drop(listener);
        }

        #[test]
        fn test_callbacks() {
            // Incoming messages are forwarded, errors unblock connect()
            let (url, accepted) = server();
            let (got_tx, got) = mpsc::channel();
            let got_tx = Mutex::new(got_tx);
            let sock = PrinterWs::new(Arc::new(move |m| got_tx.lock().unwrap().send(m).unwrap()));
            sock.connect(&url, &[]);
            let (_, to_client, _) = accepted.recv().unwrap();
            to_client.send("Jmsg".into()).unwrap();
            assert_eq!(got.recv_timeout(Duration::from_secs(5)).unwrap(), "Jmsg");

            // an error (here: nobody listening) sets the open event at once
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let refused = format!("ws://{}{URL_PATH}", listener.local_addr().unwrap());
            drop(listener);
            let started = std::time::Instant::now();
            sock.connect(&refused, &[]);
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }

    #[test]
    fn connect_send_receive_disconnect() {
        let (url, accepted) = server();
        let (got_tx, got) = mpsc::channel();
        let got_tx = Mutex::new(got_tx);
        let ws = PrinterWs::new(Arc::new(move |m| got_tx.lock().unwrap().send(m).unwrap()));
        assert!(!ws.connected());
        assert!(!ws.send(&serde_json::json!({})));

        ws.connect(
            &url,
            &[
                ("Token".into(), Some("abc".into())),
                ("Fingerprint".into(), None),
            ],
        );
        assert!(ws.connected());
        let (headers, to_client, from_client) = accepted.recv().unwrap();
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(header("token").as_deref(), Some("abc"));
        assert_eq!(header("fingerprint"), None);
        let host = url.trim_start_matches("ws://").trim_end_matches("/p/ws");
        assert_eq!(header("origin"), Some(format!("http://{host}")));

        assert!(ws.send(&serde_json::json!({"event": "INFO", "n": 1.0})));
        assert_eq!(
            from_client.recv().unwrap(),
            r#"{"event": "INFO", "n": 1.0}"#
        );
        to_client.send("J0000000a{}".into()).unwrap();
        assert_eq!(
            got.recv_timeout(Duration::from_secs(5)).unwrap(),
            "J0000000a{}"
        );

        ws.disconnect();
        assert!(!ws.connected());
        assert_eq!(
            from_client.recv_timeout(Duration::from_secs(5)).unwrap(),
            "<close>"
        );
    }

    #[test]
    fn refused_connection_returns_quickly() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/p/ws", listener.local_addr().unwrap());
        drop(listener);
        let ws = PrinterWs::new(Arc::new(|_| {}));
        let started = std::time::Instant::now();
        ws.connect(&url, &[]);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!ws.connected());
    }

    #[test]
    fn origin_header() {
        let req = |url: &str| url.into_client_request().unwrap();
        assert_eq!(
            origin(&req("wss://hub.example/p/ws")).as_deref(),
            Some("https://hub.example")
        );
        assert_eq!(
            origin(&req("ws://h:8080/p/ws")).as_deref(),
            Some("http://h:8080")
        );
        assert_eq!(origin(&req("ws://h:80/p/ws")).as_deref(), Some("http://h"));
    }
}
