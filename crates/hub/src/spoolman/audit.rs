//! The Spoolman API audit: the hub's `/api/v1` against real Spoolman's answers.
//!
//! `scripts/spoolman-audit-check.py` starts Spoolman (the one reference, MIT, at the commit named
//! in the fixture) on SQLite, sends it the request list in order and records status, headers and
//! body of every answer in `audit-fixture.json`. This test sends the same requests, in the same
//! order, to the hub on a fresh store (signed in as the tests are) and compares each answer with
//! the same normalisation (`normalisation` in the fixture). It does not change the hub to make it
//! pass: the differences it measures are written to `audit-differences.txt` and the test fails
//! when they are not exactly that list. `SPOOLMAN_AUDIT_WRITE=1 cargo test ... spoolman::audit`
//! rewrites the list; `docs/audits/spoolman.md` is written from it.
//!
//! Headers the hub adds that Spoolman does not send (`OURS_HEADERS`) are listed as ours, not as
//! differences. Fields Maker Connect adds on purpose (`OURS_FIELDS`, owner) are listed as ours,
//! not as differences.

use super::parse_iso;
use crate::store::Store;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

const FIXTURE: &str = include_str!("audit-fixture.json");
const DIFFERENCES: &str = include_str!("audit-differences.txt");

/// Headers of the hub's answers that Spoolman does not send and that are not part of the API:
/// what the hub's HTTP layer or sign-in adds to every answer.
const OURS_HEADERS: &[(&str, &str)] = &[(
    "access-control-expose-headers",
    "the hub lets a browser page read `x-total-count`; Spoolman sends it only when its CORS is configured",
)];

/// Fields Maker Connect adds to Spoolman's objects on purpose (owner's decision): `extra` keys,
/// by object path, that are ours.
const OURS_FIELDS: &[&str] = &[
    // The hub says what it is in /info.
    "$.server",
    // Maker Connect's extra fields: a filament's kind, unit, colour name, ink, media, width,
    // minimum; a spool's machine and slot.
    ".extra.kind",
    ".extra.unit",
    ".extra.color_name",
    ".extra.ink",
    ".extra.media",
    ".extra.width_mm",
    ".extra.minimum",
    ".extra.loaded_in",
    ".extra.slot",
];

fn is_ours_header(name: &str) -> bool {
    OURS_HEADERS.iter().any(|(n, _)| *n == name)
}

/// Length of an ISO 8601 date-time at the start of `s` (`T` or space between date and time,
/// optional fraction, optional `Z` or `+hh:mm`), if there is one.
fn iso_len(s: &[u8]) -> Option<usize> {
    let digit = |i: usize| s.get(i).is_some_and(u8::is_ascii_digit);
    let at = |i: usize, c: u8| s.get(i) == Some(&c);
    if !(0..19).all(|i| match i {
        4 | 7 => at(i, b'-'),
        10 => at(i, b'T') || at(i, b' '),
        13 | 16 => at(i, b':'),
        _ => digit(i),
    }) {
        return None;
    }
    let mut n = 19;
    if at(n, b'.') {
        let mut k = n + 1;
        while digit(k) {
            k += 1;
        }
        if k > n + 1 {
            n = k;
        }
    }
    if at(n, b'Z') {
        n += 1;
    } else if (at(n, b'+') || at(n, b'-'))
        && digit(n + 1)
        && digit(n + 2)
        && at(n + 3, b':')
        && digit(n + 4)
        && digit(n + 5)
    {
        n += 6;
    }
    Some(n)
}

/// A date-time as Unix seconds (no zone means UTC).
fn ts(s: &str) -> Option<i64> {
    let base = parse_iso(&s[..19].replace(' ', "T"))?;
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let off = match rest.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let h: i64 = rest[1..3].parse().ok()?;
            let m: i64 = rest[4..6].parse().ok()?;
            sign * (h * 3600 + m * 60)
        }
        _ => 0,
    };
    Some(base - off)
}

/// A date-time within 300 s of `now` is `<now>`, any other stays.
fn norm_ts(s: &str, now: i64) -> String {
    match ts(s) {
        Some(t) if (t - now).abs() <= 300 => "<now>".to_owned(),
        _ => s.to_owned(),
    }
}

fn norm_text(t: &str, now: i64) -> String {
    let b = t.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        if t.is_char_boundary(i)
            && let Some(n) = iso_len(&b[i..])
        {
            out.push_str(&norm_ts(&t[i..i + n], now));
            i += n;
            continue;
        }
        let c = t[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn norm_value(v: &mut Value, now: i64) {
    match v {
        Value::Object(m) => m.values_mut().for_each(|x| norm_value(x, now)),
        Value::Array(a) => a.iter_mut().for_each(|x| norm_value(x, now)),
        Value::String(s) if iso_len(s.as_bytes()) == Some(s.len()) => {
            *s = norm_ts(s, now);
        }
        _ => {}
    }
}

fn norm_special(path: &str, body: &mut Value) {
    let p = path.split('?').next().unwrap_or(path);
    if p == "/api/v1/info"
        && let Some(m) = body.as_object_mut()
    {
        for k in ["data_dir", "logs_dir", "backups_dir"] {
            m.insert(k.into(), "<path>".into());
        }
        for k in ["latest_version", "update_available"] {
            m.insert(k.into(), "<volatile>".into());
        }
    }
    if p == "/api/v1/backup"
        && let Some(m) = body.as_object_mut()
        && m.contains_key("path")
    {
        m.insert("path".into(), "<path>".into());
    }
}

fn norm_header(v: &str) -> String {
    // `http://127.0.0.1:<port>` becomes `http://<host>`.
    match v.find("http://127.0.0.1:") {
        Some(i) => {
            let rest = &v[i + 17..];
            let n = rest.chars().take_while(char::is_ascii_digit).count();
            format!("{}http://<host>{}", &v[..i], &rest[n..])
        }
        None => v.to_owned(),
    }
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    text: String,
}

fn send(agent: &ureq::Agent, base: &str, req: &Value) -> Answer {
    let method = req["method"].as_str().unwrap();
    let url = format!("{base}{}", req["path"].as_str().unwrap());
    let request = agent.request(method, &url);
    let response = if let Some(raw) = req.get("raw_body") {
        request
            .set("Content-Type", raw["content_type"].as_str().unwrap())
            .send_string(raw["text"].as_str().unwrap())
    } else if let Some(b) = req.get("body") {
        request.send_json(b.clone())
    } else {
        request.call()
    };
    let response = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => panic!("{method} {url}: {e}"),
    };
    let status = response.status();
    let mut headers: Vec<(String, String)> = response
        .headers_names()
        .into_iter()
        .map(|n| {
            let v = response.header(&n).unwrap_or("").to_owned();
            (n.to_lowercase(), v)
        })
        .collect();
    headers.sort();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut response.into_reader(), &mut text).ok();
    Answer {
        status,
        headers,
        text,
    }
}

fn short(v: &Value) -> String {
    short_n(v, 70)
}

fn short_n(v: &Value, n: usize) -> String {
    let s = v.to_string();
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s
    }
}

/// Where two JSON values differ, as short lines (at most `limit`, then a count).
fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let (kx, ky): (Vec<&String>, Vec<&String>) = (
                x.keys().filter(|k| y.contains_key(*k)).collect(),
                y.keys().filter(|k| x.contains_key(*k)).collect(),
            );
            if kx != ky {
                out.push(format!("{path}: key order differs"));
            }
            for (k, v) in x {
                let p = format!("{path}.{k}");
                match y.get(k) {
                    Some(w) => diff(&p, v, w, out),
                    None => out.push(format!("{p}: only Spoolman ({})", short(v))),
                }
            }
            for (k, w) in y {
                let p = format!("{path}.{k}");
                if !x.contains_key(k) {
                    if OURS_FIELDS.iter().any(|f| p.ends_with(f)) {
                        continue;
                    }
                    out.push(format!("{p}: only ours ({})", short(w)));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: {} items, ours {}", x.len(), y.len()));
            }
            for (i, (v, w)) in x.iter().zip(y).enumerate() {
                diff(&format!("{path}[{i}]"), v, w, out);
            }
        }
        _ => {
            if a != b {
                out.push(format!("{path}: Spoolman {}, ours {}", short(a), short(b)));
            }
        }
    }
}

/// `$[3].a[0].b` as `$[].a[].b`.
fn strip_indexes(s: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for c in s.chars() {
        match c {
            '[' => {
                out.push('[');
                skip = true;
            }
            ']' => {
                skip = false;
                out.push(']');
            }
            _ if skip => {}
            _ => out.push(c),
        }
    }
    out
}

/// One request's differences: empty when identical.
fn compare(req: &Value, ours: &Answer, now: i64) -> Vec<String> {
    let mut out = Vec::new();
    let status = req["status"].as_u64().unwrap() as u16;
    if status != ours.status {
        out.push(format!("status: Spoolman {status}, ours {}", ours.status));
    }
    // headers
    let ref_headers: std::collections::BTreeMap<_, _> =
        req["headers"].as_object().unwrap().iter().collect();
    let mut our_map = std::collections::BTreeMap::new();
    for (n, v) in &ours.headers {
        if n == "date" || n == "server" || is_ours_header(n) {
            continue;
        }
        our_map.insert(n.clone(), norm_header(v));
    }
    for (n, v) in &ref_headers {
        match our_map.get(n.as_str()) {
            Some(w) if Some(w.as_str()) == v.as_str() => {}
            Some(w) => out.push(format!("header {n}: Spoolman {}, ours {w}", short(v))),
            None => out.push(format!("header {n}: only Spoolman ({})", short(v))),
        }
    }
    for (n, w) in &our_map {
        if !ref_headers.contains_key(n) {
            out.push(format!("header {n}: only ours ({w})"));
        }
    }
    // body
    let path = req["path"].as_str().unwrap();
    let json = ours
        .headers
        .iter()
        .any(|(n, v)| n == "content-type" && v.contains("json"));
    if let Some(expected) = req.get("response_json") {
        if json {
            match serde_json::from_str::<Value>(&ours.text) {
                Ok(mut got) => {
                    norm_special(path, &mut got);
                    norm_value(&mut got, now);
                    if status != ours.status {
                        // Another answer altogether: the two bodies, whole.
                        out.push(format!(
                            "body: Spoolman {}, ours {}",
                            short_n(expected, 150),
                            short_n(&got, 150)
                        ));
                    } else {
                        let mut d = Vec::new();
                        diff("$", expected, &got, &mut d);
                        out.extend(d.into_iter().map(|l| format!("body {l}")));
                    }
                }
                Err(_) => out.push(format!(
                    "body: Spoolman JSON, ours not JSON ({})",
                    short(&Value::String(ours.text.clone()))
                )),
            }
        } else {
            out.push(format!(
                "body: Spoolman JSON, ours {}",
                short(&Value::String(ours.text.clone()))
            ));
        }
    } else if let Some(expected) = req.get("response_text") {
        let got = norm_text(&ours.text, now);
        if expected.as_str() != Some(got.as_str()) {
            out.push(format!(
                "body: Spoolman text {}, ours {}",
                short(expected),
                short(&Value::String(got))
            ));
        }
    } else if !ours.text.is_empty() {
        out.push(format!(
            "body: Spoolman empty, ours {}",
            short(&Value::String(ours.text.clone()))
        ));
    }
    // The same difference in every item of a list is one line with a count; fields the hub
    // sends as null where Spoolman leaves them out are one line.
    let (nulls, rest): (Vec<String>, Vec<String>) = out
        .into_iter()
        .partition(|l| l.starts_with("body ") && l.ends_with(": only ours (null)"));
    let mut out: Vec<String> = Vec::new();
    let mut seen: Vec<(String, usize)> = Vec::new();
    for l in rest {
        let key = strip_indexes(l.split_once(": ").map_or(&l, |(k, _)| k));
        match seen.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => {
                seen.push((key, 1));
                out.push(l);
            }
        }
    }
    for (line, (_, n)) in out.iter_mut().zip(&seen) {
        if *n > 1 {
            line.push_str(&format!(" (x{n})"));
        }
    }
    if !nulls.is_empty() {
        let kinds: BTreeSet<String> = nulls
            .iter()
            .map(|l| {
                let path = l[5..l.len() - ": only ours (null)".len()].to_owned();
                strip_indexes(&path)
            })
            .collect();
        out.push(format!(
            "body: ours sends {} fields as null that Spoolman leaves out ({})",
            nulls.len(),
            kinds.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    // The length follows from the body: reported only when nothing else about the body differs.
    if out.iter().any(|l| l.starts_with("body")) {
        out.retain(|l| !l.starts_with("header content-length"));
    }
    out
}

/// The vendor, filament or spool the hub made for a POST that Spoolman refused.
fn stray_id(req: &Value, ours: &Answer) -> Option<(&'static str, i64)> {
    let kind = match req["path"].as_str()? {
        "/api/v1/vendor" => "vendor",
        "/api/v1/filament" => "filament",
        "/api/v1/spool" => "spool",
        _ => return None,
    };
    if req["method"] != "POST" || ours.status != 200 || req["status"] == 200 {
        return None;
    }
    let id = serde_json::from_str::<Value>(&ours.text).ok()?["id"].as_i64()?;
    Some((kind, id))
}

/// Every request of the fixture, in order, against the hub: the lines of what differs.
fn measure() -> (usize, usize, String) {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let requests = fixture["requests"].as_array().unwrap().clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::auth::signed_in_app(store);
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let base = format!("http://{addr}");
        tokio::task::spawn_blocking(move || {
            let agent = ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(std::time::Duration::from_secs(60))
                .build();
            let mut lines = String::new();
            let mut identical = 0;
            for (i, req) in requests.iter().enumerate() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                let ours = send(&agent, &base, req);
                let d = compare(req, &ours, now);
                let head = format!(
                    "{:03} {} {} {}",
                    i + 1,
                    req["name"].as_str().unwrap(),
                    req["method"].as_str().unwrap(),
                    req["path"].as_str().unwrap()
                );
                // A thing the hub made that Spoolman refused would shift every id after it:
                // the harness deletes it through the hub's own API, outside the comparison.
                let stray = stray_id(req, &ours);
                if let Some((kind, id)) = stray {
                    send(
                        &agent,
                        &base,
                        &serde_json::json!({"method": "DELETE", "path": format!("/api/v1/{kind}/{id}")}),
                    );
                }
                let note = stray.map(|(kind, id)| {
                    format!("    (ours made {kind} {id} where Spoolman refused; deleted before the next request)\n")
                });
                if d.is_empty() && note.is_none() {
                    identical += 1;
                    lines.push_str(&format!("{head}\n    identical\n"));
                } else {
                    lines.push_str(&format!("{head}\n"));
                    lines.push_str(&note.unwrap_or_default());
                    let shown = usize::MAX;
                    for l in d.iter().take(shown) {
                        lines.push_str(&format!("    {l}\n"));
                    }
                    if d.len() > shown {
                        lines.push_str(&format!("    ... and {} more\n", d.len() - shown));
                    }
                }
            }
            (requests.len(), identical, lines)
        })
        .await
        .unwrap()
    })
}

#[test]
fn the_hub_differs_from_spoolman_exactly_as_listed() {
    let (total, identical, lines) = measure();
    let summary = format!("{identical} of {total} identical\n");
    let report = format!("{summary}{lines}");
    if std::env::var("SPOOLMAN_AUDIT_WRITE").is_ok() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/spoolman/audit-differences.txt"
        );
        std::fs::write(path, &report).unwrap();
        return;
    }
    let (want, got): (Vec<&str>, Vec<&str>) =
        (DIFFERENCES.lines().collect(), report.lines().collect());
    let changed: BTreeSet<usize> = (0..want.len().max(got.len()))
        .filter(|&i| want.get(i) != got.get(i))
        .collect();
    if let Some(&first) = changed.iter().next() {
        panic!(
            "the measured differences are not the written list (first change at line {}):\nwritten:  {:?}\nmeasured: {:?}\n(SPOOLMAN_AUDIT_WRITE=1 rewrites the list)",
            first + 1,
            want.get(first),
            got.get(first)
        );
    }
}
