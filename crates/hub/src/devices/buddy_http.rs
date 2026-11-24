//! How Buddy 8.0.2 reads the hub's HTTP answers: its parser, ported to test the hub's answers with
//! (GPL-3.0, `reference/buddy-8.0.2` at `b1043d6`; owner, 2026-11-24, #202 option A).
//!
//! - The automaton's tables are Buddy's own, made by its generator
//!   (`utils/gen-automata/http_client.py`) and kept unchanged in `buddy_http_automaton.json`
//!   (`scripts/buddy-http-automaton.py`, checked by `scripts/buddy-audit-check.py`).
//! - The engine that runs them: `src/common/automata/core.cpp` (`Transition::matches`,
//!   `Automaton::gen_event`, `Automaton::transition`, `Execution::feed`, `Execution::consume`).
//! - What Buddy keeps of the events: `src/common/http/resp_parser.cpp` (`ResponseParser::event`,
//!   `::extra`) with `resp_parser.h`'s fields, and what `HttpClient::parse_response` makes of them
//!   (`src/common/http/httpc.cpp:276-311`).
//!
//! With WEBSOCKET a MINI still talks HTTP to register (`/p/register`, `connect/registrator.cpp`)
//! and to open its socket (`/p/ws`, `connect/connect.cpp:160-173`); these are the answers tested.

use serde::Deserialize;
use std::collections::HashMap;

/// The generator's tables (`common.py` `Compiled`).
#[derive(Deserialize)]
struct Tables {
    names: HashMap<String, u16>,
    paths: Vec<String>,
    /// (target, label type, label, fallthrough).
    transitions: Vec<(u16, String, String, bool)>,
    /// (first transition, emit on enter, emit on leave); the last is the sentinel.
    states: Vec<(u16, bool, bool)>,
}

/// `ActiveState` (`core.h`): the state and the position on a path leaving it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Active {
    state: u16,
    path: u8,
}

impl Active {
    fn new(state: u16) -> Active {
        Active { state, path: 0 }
    }
}

/// `Event` (`core.h`); only what `ResponseParser::event` reads.
struct Event {
    entering_state: u16,
    payload: u8,
}

/// `ExecutionControl` (`core.h`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Control {
    Continue,
    /// `ResponseParser::event` never asks for it; kept as `core.h` names it.
    #[allow(dead_code)]
    Stop,
    NoTransition,
}

/// A label as the C++ compiler reads it: the generator writes it into a char literal, `'%s'`
/// (`common.py` `Compiled.cpp_file`), so `\\n` is a newline.
fn c_char(label: &str) -> u8 {
    match label.as_bytes() {
        [b] => *b,
        [b'\\', b'n'] => b'\n',
        [b'\\', b'r'] => b'\r',
        [b'\\', b't'] => b'\t',
        [b'\\', b'0'] => 0,
        [b'\\', c @ (b'\\' | b'\'' | b'"')] => *c,
        other => panic!(
            "a label the port does not read: {:?}",
            String::from_utf8_lossy(other)
        ),
    }
}

/// `isspace` in the C locale.
fn c_isspace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

impl Tables {
    fn load() -> Tables {
        serde_json::from_str(include_str!("buddy_http_automaton.json")).expect("the tables")
    }

    /// `Transition::matches`.
    fn matches(&self, i: usize, byte: u8) -> bool {
        let (_, kind, label, _) = &self.transitions[i];
        match kind.as_str() {
            "Char" => byte == c_char(label),
            "CharNoCase" => byte.to_ascii_lowercase() == c_char(label),
            "Special" => match label.as_str() {
                "All" => true,
                "Whitespace" => c_isspace(byte),
                "HorizWhitespace" => byte == b' ' || byte == b'\t',
                "Digit" => byte.is_ascii_digit(),
                other => panic!("no such special label {other}"),
            },
            _ => false,
        }
    }

    /// `Automaton::gen_event`.
    fn gen_event(&self, old: u16, new: u16, payload: u8) -> Option<Event> {
        let leave = self.states[old as usize].2;
        let enter = self.states[new as usize].1;
        (leave || enter).then_some(Event {
            entering_state: new,
            payload,
        })
    }

    /// `Automaton::transition`: the new state, the event, and whether to feed the byte again.
    fn transition(&self, old: Active, byte: u8) -> Option<(Active, Option<Event>, bool)> {
        let first = self.states[old.state as usize].0 as usize;
        let sentinel = self.states[old.state as usize + 1].0 as usize;
        for i in first..sentinel {
            let (target, kind, label, fallthrough) = &self.transitions[i];
            if kind == "Path" {
                // "All paths are case-insensitive."
                let converted = byte.to_ascii_lowercase();
                let path =
                    self.paths[self.paths.iter().position(|p| p == label).unwrap()].as_bytes();
                if path[old.path as usize] == converted {
                    if old.path as usize + 1 == path.len() {
                        let new = Active::new(old.state + 1);
                        return Some((new, self.gen_event(old.state, new.state, byte), false));
                    } else {
                        let mut step = old;
                        step.path += 1;
                        return Some((step, None, false));
                    }
                }
            } else if self.matches(i, byte) {
                let new = Active::new(*target);
                return Some((new, self.gen_event(old.state, *target, byte), *fallthrough));
            }
        }
        None
    }
}

/// `ResponseParser` (`resp_parser.h`), with its `Execution`.
pub(crate) struct Response {
    tables: Tables,
    names: HashMap<u16, String>,
    current: Active,
    pub status_code: u16,
    pub content_length: Option<usize>,
    pub content_type: &'static str,
    pub keep_alive: Option<bool>,
    pub content_encryption_mode: Option<&'static str>,
    /// `uint8_t version_major : 2`.
    pub version_major: u8,
    /// `uint8_t version_minor : 4`.
    pub version_minor: u8,
    pub done: bool,
    /// What `ExtraHeader::character` got, by header (`HeaderName`).
    pub extra: HashMap<&'static str, String>,
}

impl Response {
    pub(crate) fn new() -> Response {
        let tables = Tables::load();
        let names = tables.names.iter().map(|(n, s)| (*s, n.clone())).collect();
        Response {
            tables,
            names,
            current: Active::new(0),
            status_code: 0,
            content_length: None,
            content_type: "ApplicationOctetStream",
            keep_alive: None,
            content_encryption_mode: None,
            version_major: 0,
            version_minor: 0,
            done: false,
            extra: HashMap::new(),
        }
    }

    fn extra(&mut self, c: u8, name: &'static str) {
        self.extra.entry(name).or_default().push(c as char);
    }

    /// `ResponseParser::event`.
    fn event(&mut self, event: Event) -> Control {
        let Some(name) = self.names.get(&event.entering_state).cloned() else {
            return Control::Continue;
        };
        let p = event.payload;
        match name.as_str() {
            "StatusCode" => {
                self.status_code = self
                    .status_code
                    .wrapping_mul(10)
                    .wrapping_add(p.wrapping_sub(b'0') as u16)
            }
            "Body" => self.done = true,
            "ContentLength" => {
                let n = self.content_length.unwrap_or(0);
                self.content_length = Some(10 * n + (p - b'0') as usize);
            }
            "ApplicationJson" => self.content_type = "ApplicationJson",
            "TextGcodeDot" | "TextGcodeDash" => self.content_type = "TextGcode",
            "CommandId" => self.extra(p, "CommandId"),
            "Code" => self.extra(p, "Code"),
            "Token" => self.extra(p, "Token"),
            "WebSocketAccept" => self.extra(p, "WebSocketAccept"),
            "WsPrusaConnect" => self.extra(p, "WebSocketPrusaConnect"),
            "UpgradeWebsocket" => self.extra(p, "WebSocketUpgrade"),
            "ConnectionUpgrade" => self.extra(p, "ConnectionUpgrade"),
            "ConnectionHeader" => self.keep_alive = Some(false),
            "ConnectionKeepAlive" => self.keep_alive = Some(true),
            "ContentEncryptionModeCBC" => self.content_encryption_mode = Some("AES_CBC"),
            "ContentEncryptionModeCTR" => self.content_encryption_mode = Some("AES_CTR"),
            "Version" => match p {
                b'.' => {
                    self.version_major = self.version_minor & 0b11;
                    self.version_minor = 0;
                }
                b'0'..=b'9' => {
                    self.version_minor = (10 * self.version_minor + (p - b'0')) & 0b1111;
                }
                _ => {}
            },
            _ => {}
        }
        Control::Continue
    }

    /// `Execution::feed`.
    fn feed(&mut self, byte: u8) -> Control {
        let Some((new, event, refeed)) = self.tables.transition(self.current, byte) else {
            return Control::NoTransition;
        };
        self.current = new;
        if let Some(event) = event {
            let result = self.event(event);
            if result != Control::Continue {
                return result;
            }
        }
        if refeed {
            self.feed(byte)
        } else {
            Control::Continue
        }
    }

    /// `Execution::consume`: how it ended, and how many bytes were taken.
    pub(crate) fn consume(&mut self, data: &[u8]) -> (Control, usize) {
        let mut consumed = 0;
        for &b in data {
            let control = self.feed(b);
            if control != Control::NoTransition {
                consumed += 1;
            }
            if control != Control::Continue {
                return (control, consumed);
            }
        }
        (Control::Continue, consumed)
    }

    /// `Response::can_keep_alive` as `HttpClient::parse_response` sets it (`httpc.cpp:308-311`).
    pub(crate) fn can_keep_alive(&self) -> bool {
        match self.keep_alive {
            Some(k) => k,
            None => self.version_major == 1 && self.version_minor >= 1,
        }
    }
}

/// What Buddy makes of one answer, as `HttpClient::parse_response` takes it (`httpc.cpp:276-311`):
/// fed as it comes; a byte the automaton has no transition for before the head is over is
/// `Error::Parse`.
pub(crate) fn parse(answer: &[u8]) -> Result<(Response, usize), String> {
    let mut parser = Response::new();
    let (control, consumed) = parser.consume(answer);
    if !parser.done && control == Control::NoTransition {
        return Err(format!(
            "Error::Parse at byte {consumed}: {:?}",
            String::from_utf8_lossy(&answer[..(consumed + 1).min(answer.len())])
        ));
    }
    if !parser.done {
        return Err("the head never ended".into());
    }
    Ok((parser, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Buddy's own answers to itself: what its parser makes of a plain answer, one the way it
    /// asks for a command (`Command-Id`, `text/x.gcode`), and the head it ends at.
    #[test]
    fn buddys_parser_reads_an_answer() {
        let answer = b"HTTP/1.1 200 OK\r\nContent-Type: text/x.gcode\r\nCommand-Id: 42\r\nContent-Length: 3\r\nConnection: keep-alive\r\n\r\nG28";
        let (r, consumed) = parse(answer).unwrap();
        assert_eq!(
            (
                r.status_code,
                r.content_length,
                r.content_type,
                r.keep_alive
            ),
            (200, Some(3), "TextGcode", Some(true))
        );
        assert_eq!((r.version_major, r.version_minor), (1, 1));
        assert_eq!(r.extra["CommandId"], "42");
        // The head is over at the blank line, and the parser eats its `\n` too
        // (`resp_parser.cpp:33-35`).
        assert_eq!(&answer[consumed..], b"G28");
        // No Connection header: HTTP/1.1 keeps it alive, 1.0 not (`httpc.cpp:308-311`).
        let (r, _) = parse(b"HTTP/1.0 204 No Content\r\n\r\n").unwrap();
        assert_eq!((r.status_code, r.can_keep_alive()), (204, false));
        let (r, _) = parse(b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n").unwrap();
        assert_eq!(r.keep_alive, Some(false));
        assert!(parse(b"HTTX/1.1 200 OK\r\n\r\n").is_err());
    }

    /// The hub's answers as a MINI on Buddy 8.0.2 gets them, from a real server: registering
    /// (`connect/registrator.cpp`: POST `/p/register` with its JSON, chunked, as `httpc.cpp:211-223`
    /// writes it, then GET with its `Code`) and opening its socket (GET `/p/ws`,
    /// `connect/connect.cpp:160-173`). Each is read by Buddy's own parser: no parse error, its head
    /// ends, the status, the body's length, keep-alive, the HTTP version, and the headers it takes.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_hubs_answers_as_buddy_reads_them(pool: sqlx::PgPool) {
        use base64::Engine;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::app(store.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        // One request on its own connection, and the answer as far as Buddy reads it: the head,
        // then as many bytes as its Content-Length says.
        let ask = |request: Vec<u8>| async move {
            let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
            conn.write_all(&request).await.unwrap();
            let mut answer = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                if let Ok((r, consumed)) = parse(&answer)
                    && answer.len() >= consumed + r.content_length.unwrap_or(0)
                {
                    return (r, answer[consumed..].to_vec());
                }
                let n =
                    tokio::time::timeout(std::time::Duration::from_secs(10), conn.read(&mut buf))
                        .await
                        .expect("an answer in time")
                        .unwrap();
                assert!(
                    n > 0,
                    "closed before the answer was whole: {:?}",
                    String::from_utf8_lossy(&answer)
                );
                answer.extend_from_slice(&buf[..n]);
            }
        };
        let head = |method: &str, extra: &str| {
            format!(
                "{method} /p/{extra}HTTP/1.1\r\nHost: {addr}\r\nConnection: keep-alive\r\nUser-Agent-Printer: MINI\r\nUser-Agent-Version: 8.0.2+0.LOCAL\r\n"
            )
        };

        // Registering: POST with its JSON (`registrator.cpp:57-64`), chunked (`httpc.cpp:220-222`).
        let body = r#"{"sn":"CZPX1234X017XC12345","fingerprint":"0123456789abcdef","printer_type":"2.1.0","firmware":"8.0.2+0.LOCAL"}"#;
        let request = format!(
            "{}Transfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
            head("POST", "register "),
            body.len()
        );
        let (r, rest) = ask(request.into_bytes()).await;
        assert_eq!(r.status_code, 200);
        assert_eq!((r.version_major, r.version_minor), (1, 1));
        assert_eq!(r.content_length.unwrap_or(0), rest.len());
        assert!(r.can_keep_alive());
        // No Connection header: Buddy keeps the connection, as HTTP/1.1 (`httpc.cpp:308-311`).
        assert_eq!((r.keep_alive, r.content_length), (None, Some(0)));
        let code = r
            .extra
            .get("Code")
            .cloned()
            .expect("the code, in the `Code` header");
        assert_eq!(code.trim().len(), code.len(), "the code as Buddy keeps it");

        // Asking with the code: 202 while it waits, then 200 with its token.
        let poll = || format!("{}Code: {code}\r\n\r\n", head("GET", "register "));
        let (r, rest) = ask(poll().into_bytes()).await;
        assert_eq!(
            (r.status_code, r.content_length.unwrap_or(0)),
            (202, rest.len())
        );
        let space = store.default_space().await.unwrap().0;
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let (r, rest) = ask(poll().into_bytes()).await;
        assert_eq!(
            (r.status_code, r.content_length.unwrap_or(0)),
            (200, rest.len())
        );
        let token = r
            .extra
            .get("Token")
            .cloned()
            .expect("the token, in the `Token` header");

        // Its socket (`connect.cpp:160-173`): 101, the upgrade it asked for, the protocol, and the
        // accept key it checks.
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let request = format!(
            "GET /p/ws HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\nFingerprint: 0123456789abcdef\r\nToken: {token}\r\nUpgrade: websocket\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: prusa-connect\r\n\r\n"
        );
        let (r, _) = ask(request.into_bytes()).await;
        assert_eq!(r.status_code, 101);
        // `Connection: upgrade`: a Connection header, not keep-alive (`resp_parser.cpp:72-77`).
        assert_eq!(r.keep_alive, Some(false));
        let digest = ring::digest::digest(
            &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
            format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
        );
        let accept = base64::engine::general_purpose::STANDARD.encode(digest.as_ref());
        assert_eq!(r.extra.get("WebSocketAccept"), Some(&accept));
        assert!(
            r.extra.contains_key("WebSocketUpgrade"),
            "Upgrade: websocket"
        );
        assert!(
            r.extra.contains_key("ConnectionUpgrade"),
            "Connection: upgrade"
        );
        assert!(
            r.extra.contains_key("WebSocketPrusaConnect"),
            "the prusa-connect protocol"
        );
    }
}
