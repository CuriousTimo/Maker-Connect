//! `Net` on the board: plain HTTP/1.1 for the registration and one WebSocket (tungstenite) to the
//! hub, over TCP or TLS. Host, port and TLS on or off are the SDK's `[service::connect]` `hostname`,
//! `port`, `tls`, as the agent has them.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use door_controller::traits::{
    HttpRequest, HttpResponse, Method, Net, NetError, WsMessage, WsSend, WsUpgrade,
};
use tungstenite::handshake::client::generate_key;
use tungstenite::protocol::WebSocket;
use tungstenite::{Message, client::IntoClientRequest};

use crate::tls::Stream;

#[derive(Clone)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

pub struct BoardNet {
    pub server: Server,
    ws: Option<WebSocket<Stream>>,
}

impl BoardNet {
    pub fn new(server: Server) -> Self {
        Self { server, ws: None }
    }

    fn connect(&self) -> Result<Stream, NetError> {
        // The name first: a name that does not resolve is a DNS fault, not a network one; then the
        // connect with the same 10 s as reading and writing, not the whole SYN-retry series.
        let addr = (self.server.host.as_str(), self.server.port)
            .to_socket_addrs()
            .map_err(|_| NetError::Dns)?
            .next()
            .ok_or(NetError::Dns)?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(io_to_net)?;
        tcp.set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(io_to_net)?;
        tcp.set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(io_to_net)?;
        if self.server.tls {
            Stream::tls(tcp, &self.server.host)
        } else {
            Ok(Stream::Plain(tcp))
        }
    }

    fn host_header(&self) -> String {
        let default = if self.server.tls { 443 } else { 80 };
        if self.server.port == default {
            self.server.host.clone()
        } else {
            format!("{}:{}", self.server.host, self.server.port)
        }
    }
}

/// Where the board takes its build (part 7 "Updating the firmware"): the hub's, with its token.
pub const FIRMWARE_PATH: &str = "/p/door-controller/firmware";

impl BoardNet {
    /// `GET` the hub's build with the token, handing the body to `sink` piece by piece. The length
    /// must be the `Content-Length` the hub said.
    pub fn download(
        &self,
        token: &str,
        mut sink: impl FnMut(&[u8]) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        use std::io::Read;
        let mut s = self.connect().map_err(|_| "Cannot reach the hub")?;
        let head = format!(
            "GET {FIRMWARE_PATH} HTTP/1.1\r\nHost: {}\r\nToken: {token}\r\nConnection: close\r\n\r\n",
            self.host_header()
        );
        s.write_all(head.as_bytes())
            .map_err(|_| "Cannot reach the hub")?;
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line)
            .map_err(|_| "No answer from the hub")?;
        match line.split_whitespace().nth(1) {
            Some("200") => {}
            Some("404") => return Err("The hub has no build for this board"),
            _ => return Err("The hub refused the download"),
        }
        let mut length: Option<usize> = None;
        loop {
            line.clear();
            if r.read_line(&mut line)
                .map_err(|_| "No answer from the hub")?
                == 0
            {
                return Err("The download broke off");
            }
            let l = line.trim_end_matches(['\r', '\n']);
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':')
                && k.trim().eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse().ok();
            }
        }
        let length = length.ok_or("The hub sent no length")?;
        let mut buf = [0u8; 4096];
        let mut got = 0usize;
        while got < length {
            let n = r.read(&mut buf).map_err(|_| "The download broke off")?;
            if n == 0 {
                return Err("The download broke off");
            }
            sink(&buf[..n])?;
            got += n;
        }
        log::info!("downloaded {got} bytes");
        Ok(())
    }
}

fn io_to_net(e: io::Error) -> NetError {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => NetError::Timeout,
        io::ErrorKind::UnexpectedEof => NetError::UnexpectedEof,
        io::ErrorKind::ConnectionRefused => NetError::Connect,
        io::ErrorKind::NotFound => NetError::Dns,
        _ => NetError::Network,
    }
}

fn ws_to_net(e: tungstenite::Error) -> NetError {
    match e {
        tungstenite::Error::Io(e) => io_to_net(e),
        tungstenite::Error::Tls(_) => NetError::Tls,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            NetError::UnexpectedEof
        }
        tungstenite::Error::Protocol(_)
        | tungstenite::Error::Utf8(_)
        | tungstenite::Error::Http(_) => NetError::Parse,
        _ => NetError::Internal,
    }
}

impl Net for BoardNet {
    fn http(&mut self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        let mut s = self.connect()?;
        let method = match req.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let mut head = format!(
            "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            req.path,
            self.host_header()
        );
        for (k, v) in &req.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        if req.method == Method::Post {
            head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
        }
        head.push_str("\r\n");
        s.write_all(head.as_bytes()).map_err(io_to_net)?;
        s.write_all(&req.body).map_err(io_to_net)?;
        s.flush().map_err(io_to_net)?;

        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).map_err(io_to_net)?;
        let status = line
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or(NetError::Parse)?;
        let mut headers = Vec::new();
        loop {
            line.clear();
            if r.read_line(&mut line).map_err(io_to_net)? == 0 {
                return Err(NetError::UnexpectedEof);
            }
            let l = line.trim_end_matches(['\r', '\n']);
            if l.is_empty() {
                break;
            }
            let (k, v) = l.split_once(':').ok_or(NetError::Parse)?;
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
        // The registration reads headers only; the body is left with the closed connection.
        Ok(HttpResponse { status, headers })
    }

    fn invalidate(&mut self) {
        if let Some(mut ws) = self.ws.take() {
            let _ = ws.get_mut().shutdown();
        }
    }

    fn ws_upgrade(&mut self, headers: &[(String, String)]) -> Result<WsUpgrade, NetError> {
        self.invalidate();
        let scheme = if self.server.tls { "wss" } else { "ws" };
        let url = format!("{scheme}://{}/p/ws", self.host_header());
        let mut request = url.into_client_request().map_err(|_| NetError::Internal)?;
        let h = request.headers_mut();
        h.insert(
            "Sec-WebSocket-Key",
            generate_key().parse().map_err(|_| NetError::Internal)?,
        );
        for (k, v) in headers {
            let name = tungstenite::http::HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| NetError::Internal)?;
            h.insert(name, v.parse().map_err(|_| NetError::Internal)?);
        }
        let stream = self.connect()?;
        // tungstenite reads into a 128 KiB buffer by default (`ReadBuffer`); the ESP32 has not that
        // much free in one piece. The buffer's size changes nothing on the wire.
        let config = tungstenite::protocol::WebSocketConfig::default()
            .read_buffer_size(4096)
            .write_buffer_size(0);
        match tungstenite::client::client_with_config(request, stream, Some(config)) {
            Ok((ws, _response)) => {
                self.ws = Some(ws);
                Ok(WsUpgrade::Switched)
            }
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp))) => {
                Ok(WsUpgrade::Refused(resp.status().as_u16()))
            }
            // tungstenite checks Sec-WebSocket-Accept, Upgrade and Connection itself.
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Protocol(_))) => {
                Ok(WsUpgrade::Unverified)
            }
            Err(tungstenite::HandshakeError::Failure(e)) => Err(ws_to_net(e)),
            Err(tungstenite::HandshakeError::Interrupted(_)) => Err(NetError::Timeout),
        }
    }

    fn ws_send(&mut self, kind: WsSend, data: &[u8]) -> Result<(), NetError> {
        let ws = self.ws.as_mut().ok_or(NetError::Network)?;
        let msg = match kind {
            WsSend::Text => Message::Text(String::from_utf8_lossy(data).into_owned().into()),
            WsSend::Ping => Message::Ping(data.to_vec().into()),
            WsSend::Pong => Message::Pong(data.to_vec().into()),
        };
        ws.send(msg).map_err(ws_to_net)
    }

    fn ws_receive(&mut self, wait_ms: Option<u32>) -> Result<Option<WsMessage>, NetError> {
        let ws = self.ws.as_mut().ok_or(NetError::Network)?;
        // A zero timeout is not allowed on a socket; 1 ms is the shortest wait.
        let timeout = wait_ms.map(|ms| Duration::from_millis(u64::from(ms.max(1))));
        ws.get_mut()
            .tcp()
            .set_read_timeout(timeout)
            .map_err(io_to_net)?;
        let got = ws.read();
        let _ = ws
            .get_mut()
            .tcp()
            .set_read_timeout(Some(Duration::from_secs(10)));
        match got {
            Ok(Message::Text(t)) => Ok(Some(WsMessage::Data(t.as_bytes().to_vec()))),
            Ok(Message::Binary(b)) => Ok(Some(WsMessage::Data(b.to_vec()))),
            Ok(Message::Ping(p)) => Ok(Some(WsMessage::Ping(p.to_vec()))),
            Ok(Message::Pong(_)) => Ok(Some(WsMessage::Pong)),
            Ok(Message::Close(_)) => Ok(Some(WsMessage::Close)),
            Ok(Message::Frame(_)) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(ws_to_net(e)),
        }
    }

    fn ws_readable(&mut self, ms: u32) -> bool {
        let Some(ws) = self.ws.as_mut() else {
            return false;
        };
        if ws.get_ref().buffered() {
            return true;
        }
        let tcp = ws.get_mut().tcp();
        if tcp
            .set_read_timeout(Some(Duration::from_millis(u64::from(ms.max(1)))))
            .is_err()
        {
            return false;
        }
        let mut b = [0u8; 1];
        let ready = matches!(tcp.peek(&mut b), Ok(n) if n > 0);
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
        ready
    }
}
