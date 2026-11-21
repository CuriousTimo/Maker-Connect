//! Various utilities for the Printer SDK project (`util.py`), and the parts of Python and
//! `requests` the SDK's wire output depends on: `str(float)`, `json.dumps`, truthiness, and the
//! `requests.Session` that `RetryingSession` subclasses.
//!
//! `requests` is the SDK's dependency (`requirements.txt`: `requests>=2.32.3`), not part of the
//! reference commit; its behaviour here is ported from requests 2.32 and checked in step A4:
//! - the default headers of `requests.utils.default_headers()` without `brotli` or `zstandard`
//!   installed (neither is in the SDK's requirements);
//! - the CA store: `requests` verifies against `certifi`, the Mozilla bundle; ureq's default
//!   `webpki-roots` is the same Mozilla bundle;
//! - proxies from the environment (`trust_env`), as ureq's `try_proxy_from_env`; `.netrc`
//!   credentials and `REQUESTS_CA_BUNDLE` are not read (not covered);
//! - `gzip` and `deflate` bodies decoded, the body read without a size limit;
//! - `.text`: the `charset` of `Content-Type`, ISO-8859-1 for a `text/*` type without one,
//!   otherwise UTF-8. `requests` guesses the last case with `charset_normalizer` (not covered).

use std::fmt::Write as _;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::clock;
use crate::consts;

/// If given None, gets the current timestamp, otherwise leaves the value alone.
///
/// `timestamp or int(time() * 10) * const.TIMESTAMP_PRECISION`: a timestamp of `0.0` is falsy,
/// so it is replaced too.
pub fn get_timestamp(timestamp: Option<f64>) -> f64 {
    match timestamp {
        Some(timestamp) if timestamp != 0.0 => timestamp,
        _ => (clock::time() * 10.0).trunc() * consts::TIMESTAMP_PRECISION,
    }
}

/// Uses sha256 to hash the supplied identifier for use as a fingerprint
pub fn make_fingerprint(identifier: &str) -> String {
    let digest = Sha256::digest(identifier.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Python's `str(float)` / `repr(float)`: the shortest digits that round-trip, as fixed point
/// with at least one decimal when the decimal exponent is in -4..16, otherwise as `1e+16`.
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    // Rust's `{:e}` gives the same shortest round-trip digits as Python's repr.
    let sci = format!("{x:e}");
    let (sign, sci) = match sci.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", sci.as_str()),
    };
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();

    if !(-4..16).contains(&exp) {
        let mut out = format!("{sign}{}", &digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let exp_sign = if exp < 0 { '-' } else { '+' };
        let _ = write!(out, "e{exp_sign}{:02}", exp.abs());
        return out;
    }
    if exp < 0 {
        let zeros = "0".repeat((-exp - 1) as usize);
        return format!("{sign}0.{zeros}{digits}");
    }
    let int_len = exp as usize + 1;
    if digits.len() <= int_len {
        let zeros = "0".repeat(int_len - digits.len());
        format!("{sign}{digits}{zeros}.0")
    } else {
        format!("{sign}{}.{}", &digits[..int_len], &digits[int_len..])
    }
}

/// Python's `bool(value)` for a JSON value: `None`, `False`, `0`, `0.0`, `""`, `[]` and `{}`
/// are falsy.
pub fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python's `json.dumps(value)` with its defaults: separators `", "` and `": "`,
/// `ensure_ascii=True`, keys in insertion order, floats as `repr(float)`.
pub fn py_json_dumps(value: &Value) -> String {
    let mut out = String::new();
    dump_value(&mut out, value);
    out
}

fn dump_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            if n.is_f64() {
                out.push_str(&py_float_repr(n.as_f64().expect("an f64 number")));
            } else {
                out.push_str(&n.to_string());
            }
        }
        Value::String(s) => dump_str(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_str(out, key);
                out.push_str(": ");
                dump_value(out, item);
            }
            out.push('}');
        }
    }
}

/// `py_encode_basestring_ascii`: everything outside `' '..='~'` is escaped.
fn dump_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// The `requests` exceptions the SDK tells apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// `requests.exceptions.ConnectionError`: DNS, refused, connect timeout, TLS, the remote
    /// closing its end.
    ConnectionError(String),
    /// `requests.exceptions.ReadTimeout`
    ReadTimeout(String),
    /// Any other `requests.exceptions.RequestException`.
    Other(String),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::ConnectionError(msg)
            | RequestError::ReadTimeout(msg)
            | RequestError::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for RequestError {}

/// A `requests.Response`, read in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status_code: u16,
    pub headers: Vec<(String, String)>,
    pub text: String,
}

impl Response {
    /// `res.headers[name]`: case-insensitive.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// `requests.Session`: the headers `requests` adds to every request (`User-Agent` is always
/// given by the SDK), a 30-redirect limit, and a socket timeout per call.
#[derive(Debug, Clone)]
pub struct Session {
    agent: ureq::Agent,
}

/// `requests.utils.default_headers()`, without its `User-Agent`, which the SDK always sets.
const DEFAULT_HEADERS: [(&str, &str); 3] = [
    ("Accept-Encoding", "gzip, deflate"),
    ("Accept", "*/*"),
    ("Connection", "keep-alive"),
];

impl Session {
    pub fn new() -> Session {
        let timeout = Duration::from_secs(consts::CONNECTION_TIMEOUT);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .redirects(30)
            .try_proxy_from_env(true)
            .build();
        Session { agent }
    }

    /// `Session.request(method, url, headers=headers, json=json, timeout=CONNECTION_TIMEOUT)`
    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        json: Option<&Value>,
    ) -> Result<Response, RequestError> {
        let mut request = self.agent.request(method, url);
        for (name, value) in DEFAULT_HEADERS {
            request = request.set(name, value);
        }
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let result = match json {
            Some(json) => {
                if !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("Content-Type"))
                {
                    request = request.set("Content-Type", "application/json");
                }
                request.send_bytes(py_json_dumps(json).as_bytes())
            }
            None if method != "GET" && method != "HEAD" => {
                request.set("Content-Length", "0").call()
            }
            None => request.call(),
        };
        let response = match result {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => return Err(transport_error(&transport)),
        };
        let status_code = response.status();
        let headers = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                let value = response.header(&name)?.to_owned();
                Some((name, value))
            })
            .collect();
        let encoding = response
            .header("Content-Encoding")
            .map(str::to_ascii_lowercase);
        let content_type = response.header("Content-Type").map(str::to_owned);
        let mut body = Vec::new();
        std::io::Read::read_to_end(&mut response.into_reader(), &mut body)
            .map_err(|err| io_error(&err))?;
        if encoding.as_deref() == Some("deflate") {
            body = inflate(&body).map_err(|err| io_error(&err))?;
        }
        let text = decode_text(&body, content_type.as_deref());
        Ok(Response {
            status_code,
            headers,
            text,
        })
    }
}

/// A `requests.Response` of `stream=True`: its status and headers, the body read as it is
/// iterated (`iter_content`), decoded as `requests` decodes it (`gzip` by ureq, `deflate` as
/// urllib3's `DeflateDecoder`).
pub struct StreamResponse {
    pub status_code: u16,
    pub headers: Vec<(String, String)>,
    body: Box<dyn std::io::Read + Send>,
}

impl StreamResponse {
    /// `res.headers.get(name)`: case-insensitive.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The next chunk of `iter_content(chunk_size)`: urllib3's `stream(amt)` reads `amt`
    /// decoded bytes, fewer only at the end; `None` at the end.
    pub fn next_chunk(&mut self, chunk_size: usize) -> Option<Result<Vec<u8>, RequestError>> {
        let mut chunk = vec![0u8; chunk_size];
        let mut filled = 0;
        while filled < chunk_size {
            match self.body.read(&mut chunk[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => return Some(Err(io_error(&err))),
            }
        }
        if filled == 0 {
            return None;
        }
        chunk.truncate(filled);
        Some(Ok(chunk))
    }
}

impl Session {
    /// `requests.get(url, stream=True, headers=headers, timeout=CONNECTION_TIMEOUT)`: a new
    /// `Session` for the call, as `requests.get` makes one.
    pub fn get_stream(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<StreamResponse, RequestError> {
        let mut request = self.agent.request("GET", url);
        for (name, value) in DEFAULT_HEADERS {
            request = request.set(name, value);
        }
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let response = match request.call() {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => return Err(transport_error(&transport)),
        };
        let status_code = response.status();
        let headers: Vec<(String, String)> = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                let value = response.header(&name)?.to_owned();
                Some((name, value))
            })
            .collect();
        let deflate = response
            .header("Content-Encoding")
            .is_some_and(|e| e.eq_ignore_ascii_case("deflate"));
        let mut body: Box<dyn std::io::Read + Send> = Box::new(response.into_reader());
        if deflate {
            let mut raw = Vec::new();
            std::io::Read::read_to_end(&mut body, &mut raw).map_err(|err| io_error(&err))?;
            body = Box::new(std::io::Cursor::new(
                inflate(&raw).map_err(|err| io_error(&err))?,
            ));
        }
        Ok(StreamResponse {
            status_code,
            headers,
            body,
        })
    }
}

impl Default for Session {
    fn default() -> Session {
        Session::new()
    }
}

/// urllib3's `DeflateDecoder`: zlib-wrapped first, raw deflate if that fails.
fn inflate(body: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    if flate2::read::ZlibDecoder::new(body)
        .read_to_end(&mut out)
        .is_ok()
    {
        return Ok(out);
    }
    out.clear();
    flate2::read::DeflateDecoder::new(body).read_to_end(&mut out)?;
    Ok(out)
}

/// `Response.text`, see the module doc.
fn decode_text(body: &[u8], content_type: Option<&str>) -> String {
    let content_type = content_type.unwrap_or("").to_ascii_lowercase();
    let charset = content_type
        .split(';')
        .skip(1)
        .find_map(|param| param.trim().strip_prefix("charset="))
        .map(|c| c.trim_matches(['"', '\'']).to_owned());
    let latin1 = match charset.as_deref() {
        Some("iso-8859-1" | "latin-1" | "latin1") => true,
        Some(_) => false,
        None => content_type.trim_start().starts_with("text/"),
    };
    if latin1 {
        body.iter().map(|b| char::from(*b)).collect()
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

fn io_error(err: &std::io::Error) -> RequestError {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            RequestError::ReadTimeout(err.to_string())
        }
        _ => RequestError::ConnectionError(err.to_string()),
    }
}

fn transport_error(transport: &ureq::Transport) -> RequestError {
    use ureq::ErrorKind;
    let msg = transport.to_string();
    match transport.kind() {
        // A connect timeout is `ConnectionFailed` too (ureq `stream.rs:424-426`), as requests'
        // `ConnectTimeout` is a `ConnectionError`.
        ErrorKind::Dns
        | ErrorKind::ConnectionFailed
        | ErrorKind::ProxyConnect
        | ErrorKind::BadStatus
        | ErrorKind::BadHeader => RequestError::ConnectionError(msg),
        ErrorKind::Io => {
            let timed_out = std::error::Error::source(transport)
                .and_then(|source| source.downcast_ref::<std::io::Error>())
                .is_some_and(|io| {
                    matches!(
                        io.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    )
                });
            if timed_out {
                RequestError::ReadTimeout(msg)
            } else {
                RequestError::ConnectionError(msg)
            }
        }
        _ => RequestError::Other(msg),
    }
}

/// Retry a GET/POST request in case the other ends closes the connection.
///
/// NOTE This class was added to fix mysterious occurrences of ConnectionErrors, which could
/// neither be reproduced nor amended by using urllib Retry class. Consider it a working fix
/// until the problem is investigated more deeply and an eventually cleaner solution is found.
///
/// Only `get` and `post` retry; `request` is the plain `Session.request`.
#[derive(Debug, Clone)]
pub struct RetryingSession {
    pub session: Session,
    pub max_retries: u32,
}

impl RetryingSession {
    pub fn new(max_retries: u32) -> RetryingSession {
        RetryingSession {
            session: Session::new(),
            max_retries,
        }
    }

    /// Try executing `callback` up to `self.max_retries`; if every call fails with a connection
    /// error, the last one is propagated.
    pub fn call_and_retry<T>(
        &self,
        mut callback: impl FnMut() -> Result<T, RequestError>,
    ) -> Result<T, RequestError> {
        let mut count = 0;
        let mut error = None;
        while count < self.max_retries {
            match callback() {
                Err(err @ RequestError::ConnectionError(_)) => {
                    log::info!("probably the remote closed its end");
                    error = Some(err);
                }
                other => return other,
            }
            count += 1;
        }
        // Python raises `None` here (a TypeError) when max_retries is 0.
        Err(error.unwrap_or_else(|| {
            RequestError::Other("exceptions must derive from BaseException".to_owned())
        }))
    }

    pub fn get(&self, url: &str, headers: &[(String, String)]) -> Result<Response, RequestError> {
        self.call_and_retry(|| self.session.request("GET", url, headers, None))
    }

    pub fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        json: Option<&Value>,
    ) -> Result<Response, RequestError> {
        self.call_and_retry(|| self.session.request("POST", url, headers, json))
    }

    /// `Session.request`, not retried.
    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        json: Option<&Value>,
    ) -> Result<Response, RequestError> {
        self.session.request(method, url, headers, json)
    }
}

impl Default for RetryingSession {
    fn default() -> RetryingSession {
        RetryingSession::new(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    /// Expected values printed by Python 3.11: `str(x)`.
    #[test]
    fn float_repr_is_pythons() {
        let cases: &[(f64, &str)] = &[
            (1.0, "1.0"),
            (0.1, "0.1"),
            (-2.5, "-2.5"),
            (17636000001.0 * 0.1, "1763600000.1000001"),
            (1763600000.0, "1763600000.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.25e-7, "1.25e-07"),
            (1e100, "1e+100"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (f64::INFINITY, "inf"),
            (f64::NAN, "nan"),
        ];
        for (x, want) in cases {
            assert_eq!(py_float_repr(*x), *want, "{x:?}");
        }
    }

    #[test]
    fn get_timestamp_keeps_a_truthy_one() {
        assert_eq!(get_timestamp(Some(12.5)), 12.5);
        let now = get_timestamp(None);
        assert!((now - clock::time()).abs() < 1.0);
        // a whole number of tenths
        assert_eq!(((now / consts::TIMESTAMP_PRECISION).round() * 0.1), now);
        assert_ne!(get_timestamp(Some(0.0)), 0.0);
    }

    /// `hashlib.sha256(b"abc").hexdigest()`
    #[test]
    fn fingerprint() {
        assert_eq!(
            make_fingerprint("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn truthiness() {
        for falsy in [
            json!(null),
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!py_truthy(&falsy), "{falsy}");
        }
        for truthy in [
            json!(true),
            json!(-1),
            json!(0.5),
            json!("0"),
            json!([0]),
            json!({"a": null}),
        ] {
            assert!(py_truthy(&truthy), "{truthy}");
        }
    }

    /// Expected values printed by Python 3.11: `json.dumps(x)`.
    #[test]
    fn json_dumps_is_pythons() {
        let value = json!({
            "event": "INFO",
            "data": {"temp": 21.0, "n": 3, "list": [1, 2.5, null, true], "empty": {}},
            "text": "a\"b\\c\n\t\u{7f}é😀",
        });
        assert_eq!(
            py_json_dumps(&value),
            r#"{"event": "INFO", "data": {"temp": 21.0, "n": 3, "list": [1, 2.5, null, true], "empty": {}}, "text": "a\"b\\c\n\t\u007f\u00e9\ud83d\ude00"}"#
        );
        assert_eq!(py_json_dumps(&json!([])), "[]");
        assert_eq!(py_json_dumps(&json!(1e16)), "1e+16");
    }

    #[test]
    fn body_decoding() {
        use std::io::Write;
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(b"hello").unwrap();
        assert_eq!(inflate(&zlib.finish().unwrap()).unwrap(), b"hello");
        let mut raw =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw.write_all(b"hello").unwrap();
        assert_eq!(inflate(&raw.finish().unwrap()).unwrap(), b"hello");

        let bytes = "é".as_bytes();
        assert_eq!(decode_text(bytes, Some("application/json")), "é");
        assert_eq!(decode_text(bytes, Some("text/plain")), "Ã©");
        assert_eq!(decode_text(bytes, Some("text/plain; charset=utf-8")), "é");
        assert_eq!(
            decode_text(bytes, Some("application/json; charset=ISO-8859-1")),
            "Ã©"
        );
    }

    #[test]
    fn retries_only_connection_errors() {
        let session = RetryingSession::new(3);
        let calls = Cell::new(0);
        let result: Result<(), _> = session.call_and_retry(|| {
            calls.set(calls.get() + 1);
            Err(RequestError::ConnectionError("reset".into()))
        });
        assert_eq!(calls.get(), 3);
        assert_eq!(result, Err(RequestError::ConnectionError("reset".into())));

        calls.set(0);
        let result: Result<(), _> = session.call_and_retry(|| {
            calls.set(calls.get() + 1);
            Err(RequestError::ReadTimeout("slow".into()))
        });
        assert_eq!(calls.get(), 1);
        assert_eq!(result, Err(RequestError::ReadTimeout("slow".into())));

        calls.set(0);
        let result = session.call_and_retry(|| {
            calls.set(calls.get() + 1);
            if calls.get() < 2 {
                Err(RequestError::ConnectionError("reset".into()))
            } else {
                Ok(7)
            }
        });
        assert_eq!((calls.get(), result), (2, Ok(7)));
    }
}

/// `tests/test_util.py` at `c0d7d233`, test by test. `requests_mock` is a local HTTP server.
#[cfg(test)]
mod sdk_tests {
    use super::*;
    use std::cell::Cell;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    #[test]
    fn test_get_timestamp() {
        // A given timestamp is kept, none means the current time
        assert_eq!(get_timestamp(Some(42.0)), 42.0);
        assert!(get_timestamp(None) > 0.0);
    }

    #[test]
    fn test_make_fingerprint() {
        // The fingerprint is a sha256 hex digest
        assert_eq!(
            make_fingerprint("a"),
            "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"
        );
    }

    #[test]
    fn test_call_and_retry_succeeds_after_error() {
        // A connection error is retried
        let calls = Cell::new(0);
        let result = RetryingSession::default().call_and_retry(|| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Err(RequestError::ConnectionError(String::new()))
            } else {
                Ok("ok")
            }
        });
        assert_eq!(result, Ok("ok"));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_call_and_retry_gives_up() {
        // After max_retries the last error is raised
        let calls = Cell::new(0);
        let result: Result<(), _> = RetryingSession::new(2).call_and_retry(|| {
            calls.set(calls.get() + 1);
            Err(RequestError::ConnectionError(String::new()))
        });
        assert!(matches!(result, Err(RequestError::ConnectionError(_))));
        assert_eq!(calls.get(), 2);
    }

    /// Answers each request with its method's text, as `requests_mock.get/post(text=...)`.
    fn mock_server() -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let text = if head.starts_with("GET") { "g" } else { "p" };
                let _ = tx.send(format!("{head}{}", String::from_utf8_lossy(&body)));
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n{text}").unwrap();
            }
        });
        (url, rx)
    }

    #[test]
    fn test_get_and_post() {
        // get and post go through the retrying wrapper
        let (url, requests) = mock_server();
        let session = RetryingSession::default();
        assert_eq!(session.get(&url, &[]).unwrap().text, "g");
        let a = serde_json::json!({"a": 1});
        assert_eq!(session.post(&url, &[], Some(&a)).unwrap().text, "p");
        let _get = requests.recv().unwrap();
        let post = requests.recv().unwrap();
        assert!(
            post.contains("Content-Type: application/json\r\n"),
            "{post}"
        );
        assert!(post.ends_with("\r\n\r\n{\"a\": 1}"), "{post}");
    }
}
