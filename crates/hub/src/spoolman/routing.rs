//! How Spoolman's app routes a request, before any handler: Starlette 1.10.0's `Router.app`
//! (`starlette/routing.py`, BSD-3-Clause) on Spoolman's routes in their order, and FastAPI's
//! validation of the path parameters (`int`, an `Enum`), Pydantic 2's errors. Spoolman's API
//! is mounted at `/spoolman/api/v1` (`SPOOLMAN_BASE_PATH=/spoolman`, `docs/design/api.md`).
//!
//! The routes come from Spoolman's own app: `routing.json`, written by
//! `scripts/spoolman-audit-check.py` from `spoolman.api.v1.router.app.routes`.
//!
//! - A route whose path and method match: its path parameters are validated (a 422 with
//!   Pydantic's `detail` when one cannot be taken), an `int` written back as its number so the
//!   hub's handler reads it, and the request goes on.
//! - The first route whose path matches but not its method: 405 `{"detail":"Method Not
//!   Allowed"}` with its `allow`.
//! - None: `redirect_slashes`, a 307 to the path with its trailing `/` taken off (all of them,
//!   `rstrip("/")`) or one added, when a route matches that; else 404 `{"detail":"Not Found"}`.

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::sync::LazyLock;

/// Where Spoolman's API is: its base path and `/api/v1`.
pub const MOUNT: &str = "/spoolman/api/v1";

enum Kind {
    Str,
    /// Pydantic's `int_parsing`, with its message.
    Int(String),
    /// Pydantic's `enum`: its message, `ctx`, and the values.
    Enum(String, Value, Vec<String>),
}

struct Param {
    name: String,
    kind: Kind,
}

struct Route {
    regex: regex::Regex,
    methods: Vec<String>,
    params: Vec<Param>,
}

/// Spoolman's HTTP routes in order. Its WebSocket routes never match an HTTP request
/// (`WebSocketRoute.matches` wants `scope["type"] == "websocket"`), so they are left out.
static ROUTES: LazyLock<Vec<Route>> = LazyLock::new(|| {
    let table: Value =
        serde_json::from_str(include_str!("routing.json")).expect("routing.json is JSON");
    table["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .filter(|r| r.get("websocket").is_none())
        .map(|r| Route {
            regex: regex::Regex::new(r["regex"].as_str().expect("regex")).expect("a regex"),
            methods: r["methods"]
                .as_array()
                .expect("methods")
                .iter()
                .map(|m| m.as_str().unwrap_or_default().to_owned())
                .collect(),
            params: r["params"]
                .as_array()
                .expect("params")
                .iter()
                .map(|p| Param {
                    name: p["name"].as_str().unwrap_or_default().to_owned(),
                    kind: match p["type"].as_str() {
                        Some("int_parsing") => {
                            Kind::Int(p["msg"].as_str().unwrap_or_default().into())
                        }
                        Some("enum") => Kind::Enum(
                            p["msg"].as_str().unwrap_or_default().into(),
                            p["ctx"].clone(),
                            p["values"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect(),
                        ),
                        _ => Kind::Str,
                    },
                })
                .collect(),
        })
        .collect()
});

/// uvicorn's `scope["path"]`: the path percent-decoded (`urllib.parse.unquote`, UTF-8, a
/// bad sequence replaced).
fn unquote(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Python's `urllib.parse.quote(url, safe=":/%#?=@[]!$&'()*+,;")`, as Starlette's
/// `RedirectResponse` quotes its `location`.
fn quote(url: &str) -> String {
    const SAFE: &str = ":/%#?=@[]!$&'()*+,;";
    let mut out = String::with_capacity(url.len());
    for b in url.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || SAFE.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Pydantic 2's `int` from a string (pydantic-core `str_as_int`), as measured on Spoolman's
/// Pydantic 2.14.2: whitespace around it, a sign, `_` between digits, and a `.` with only
/// zeros after it are taken (`" 1"`, `"+1"`, `"1_000"`, `"1.0"`); `"1.5"`, `"1."`, `"0x10"`,
/// `"1e3"` are not. `None`: not an int. A number past `i64` is not taken here.
pub fn pydantic_int(s: &str) -> Option<i64> {
    let s = s.trim();
    let s = match s.split_once('.') {
        Some((int, zeros)) if !zeros.is_empty() && zeros.bytes().all(|b| b == b'0') => int,
        Some(_) => return None,
        None => s,
    };
    if let Ok(i) = s.parse::<i64>() {
        return Some(i);
    }
    // Underscores only between digits.
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'+' | b'-') => s.split_at(1),
        _ => ("", s),
    };
    if digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    format!("{sign}{}", digits.replace('_', "")).parse().ok()
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (status, axum::Json(body)).into_response()
}

/// The layer, for every request of the hub: only those under [`MOUNT`] are Spoolman's.
pub async fn routing(mut req: Request, next: Next) -> Response {
    let full_path = unquote(req.uri().path());
    let Some(rest) = full_path
        .strip_prefix(MOUNT)
        .and_then(|r| r.strip_prefix('/'))
    else {
        return next.run(req).await;
    };
    let websocket = req
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if websocket {
        return next.run(req).await;
    }
    let route_path = format!("/{rest}");
    let method = req.method().as_str().to_owned();
    let mut partial: Option<&Route> = None;
    for route in ROUTES.iter() {
        let Some(caps) = route.regex.captures(&route_path) else {
            continue;
        };
        if !route.methods.contains(&method) {
            partial = partial.or(Some(route));
            continue;
        }
        // Match.FULL: FastAPI validates the path parameters.
        let mut errors = Vec::new();
        let mut rewritten = route_path.clone();
        let mut shift: isize = 0;
        for p in &route.params {
            let Some(m) = caps.name(&p.name) else {
                continue;
            };
            let input = m.as_str();
            match &p.kind {
                Kind::Str => {}
                Kind::Int(msg) => match pydantic_int(input) {
                    Some(n) if n.to_string() != input => {
                        let start = (m.start() as isize + shift) as usize;
                        let end = (m.end() as isize + shift) as usize;
                        let n = n.to_string();
                        shift += n.len() as isize - input.len() as isize;
                        rewritten.replace_range(start..end, &n);
                    }
                    Some(_) => {}
                    None => errors.push(json!({
                        "type": "int_parsing", "loc": ["path", p.name], "msg": msg, "input": input,
                    })),
                },
                Kind::Enum(msg, ctx, values) => {
                    if !values.iter().any(|v| v == input) {
                        errors.push(json!({
                            "type": "enum", "loc": ["path", p.name], "msg": msg, "input": input,
                            "ctx": ctx,
                        }));
                    }
                }
            }
        }
        if !errors.is_empty() {
            return json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": errors}));
        }
        if rewritten != route_path {
            let path = quote(&format!("{MOUNT}{rewritten}"));
            let uri = match req.uri().query() {
                Some(q) => format!("{path}?{q}"),
                None => path,
            };
            if let Ok(uri) = uri.parse() {
                *req.uri_mut() = uri;
            }
        }
        return next.run(req).await;
    }
    if let Some(route) = partial {
        let mut response = json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"detail": "Method Not Allowed"}),
        );
        if let Ok(allow) = HeaderValue::from_str(&route.methods.join(", ")) {
            response.headers_mut().insert("allow", allow);
        }
        return response;
    }
    if route_path != "/" {
        let (redirect_full, redirect_route) = if route_path.ends_with('/') {
            (
                full_path.trim_end_matches('/').to_owned(),
                route_path.trim_end_matches('/').to_owned(),
            )
        } else {
            (format!("{full_path}/"), format!("{route_path}/"))
        };
        // The mount itself must still match the new path: `/spoolman/api/v1/` and below.
        let mounted = redirect_full.starts_with(&format!("{MOUNT}/"));
        if mounted && ROUTES.iter().any(|r| r.regex.is_match(&redirect_route)) {
            let host = req
                .headers()
                .get("host")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            let mut url = format!("http://{host}{redirect_full}");
            if let Some(q) = req.uri().query().filter(|q| !q.is_empty()) {
                url.push('?');
                url.push_str(q);
            }
            let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
            if let Ok(location) = HeaderValue::from_str(&quote(&url)) {
                response.headers_mut().insert("location", location);
            }
            response
                .headers_mut()
                .insert("content-length", HeaderValue::from_static("0"));
            return response;
        }
    }
    json_response(StatusCode::NOT_FOUND, json!({"detail": "Not Found"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints_as_pydantic_takes_them() {
        // Measured on Spoolman's venv: TypeAdapter(int).validate_python(s).
        for (s, want) in [
            ("1", Some(1)),
            ("+1", Some(1)),
            ("-1", Some(-1)),
            (" 1", Some(1)),
            ("1 ", Some(1)),
            ("1_000", Some(1000)),
            ("1.0", Some(1)),
            ("1.5", None),
            ("01", Some(1)),
            ("0x10", None),
            ("1e3", None),
            ("", None),
            ("1.", None),
            (".0", None),
            ("-0", Some(0)),
            ("1.00000", Some(1)),
            ("1.0e0", None),
            ("\t1\n", Some(1)),
            ("1__0", None),
            ("_1", None),
        ] {
            assert_eq!(pydantic_int(s), want, "{s:?}");
        }
    }

    #[test]
    fn quoted_and_unquoted_as_python() {
        assert_eq!(unquote("/a%20b/%2F/%zz"), "/a b///%zz");
        assert_eq!(quote("http://h/a b?x=1&y=é"), "http://h/a%20b?x=1&y=%C3%A9");
    }
}
