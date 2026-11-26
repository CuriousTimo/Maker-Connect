//! How Spoolman's app routes a request, before any handler: Starlette 1.10.0's `Router.app`
//! (`starlette/routing.py`, BSD-3-Clause) on Spoolman's routes in their order, then FastAPI's
//! handling of the matched route ([`super::pydantic::solve`]). Spoolman's API is mounted at
//! `/spoolman/api/v1` (`SPOOLMAN_BASE_PATH=/spoolman`, `docs/design/api.md`).
//!
//! The routes come from Spoolman's own app: `routing.json`, written by
//! `scripts/spoolman-audit-check.py` from `spoolman.api.v1.router.app.routes`.
//!
//! - A route whose path and method match: its body read, its dependencies run, its path and
//!   query parameters and its body validated (one 422 with Pydantic's `detail` for all their
//!   errors); the result on the request for the handler ([`super::pydantic::Solved`]), an
//!   `int` path parameter written back as its number for the hub's `Path` extractors.
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

struct Route {
    regex: regex::Regex,
    methods: Vec<String>,
    /// Spoolman's own path, `/vendor/{vendor_id}`.
    path: String,
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
            path: r["path"].as_str().unwrap_or_default().to_owned(),
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

fn json_response(status: StatusCode, body: Value) -> Response {
    (status, axum::Json(body)).into_response()
}

/// The route [`routing`] matched: Spoolman's path of it and its path values (unquoted).
#[derive(Clone)]
struct Matched {
    path: String,
    values: Vec<(String, String)>,
}

/// FastAPI's handling of the route [`routing`] matched ([`super::pydantic::solve`]), the
/// result on the request for the handler: a route layer of Spoolman's routes, so inside the
/// hub's own sign-in and permission checks.
pub async fn fastapi(req: Request, next: Next) -> Response {
    let Some(matched) = req.extensions().get::<Matched>().cloned() else {
        return next.run(req).await;
    };
    let method = req.method().as_str().to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    let (mut parts, body) = req.into_parts();
    // FastAPI reads the body only for a route that has one.
    let (bytes, body) = if super::pydantic::has_body(&method, &matched.path) {
        match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => (Some(bytes), None),
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        }
    } else {
        (None, Some(body))
    };
    let solved = match super::pydantic::solve(
        &method,
        &matched.path,
        &matched.values,
        &query,
        &parts.headers,
        bytes.as_deref(),
    ) {
        Ok(solved) => solved,
        Err(response) => return response,
    };
    parts.extensions.insert(solved);
    let body = body.unwrap_or_else(|| axum::body::Body::from(bytes.unwrap_or_default()));
    next.run(Request::from_parts(parts, body)).await
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
        // Match.FULL: the route and its path values, for FastAPI's handling inside the hub's
        // permission check ([`fastapi`]); an `int` path parameter written back as its number
        // (as Pydantic takes it), for the hub's `Path` extractors.
        let values: Vec<(String, String)> = route
            .regex
            .capture_names()
            .flatten()
            .filter_map(|n| Some((n.to_owned(), caps.name(n)?.as_str().to_owned())))
            .collect();
        let mut rewritten = route_path.clone();
        let mut shift: isize = 0;
        for (name, input) in &values {
            let Some(n) = super::pydantic::path_number(&method, &route.path, name, input) else {
                continue;
            };
            if n != *input
                && let Some(m) = caps.name(name)
            {
                let start = (m.start() as isize + shift) as usize;
                let end = (m.end() as isize + shift) as usize;
                shift += n.len() as isize - input.len() as isize;
                rewritten.replace_range(start..end, &n);
            }
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
        req.extensions_mut().insert(Matched {
            path: route.path.clone(),
            values,
        });
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
    fn quoted_and_unquoted_as_python() {
        assert_eq!(unquote("/a%20b/%2F/%zz"), "/a b///%zz");
        assert_eq!(quote("http://h/a b?x=1&y=é"), "http://h/a%20b?x=1&y=%C3%A9");
    }
}
