//! Signing in: the guard in front of every route, and the pages to sign in, set up the first
//! account and sign out.
//!
//! - Machines are not people: agents, Prusa printers and Moonraker (SimplyPrint) prove who they
//!   are with their own tokens, so their routes ([`public`]) pass the guard untouched.
//! - Everything else needs a session: a random token in an HttpOnly, SameSite=Lax cookie,
//!   kept as a hash ([`crate::accounts`]). A page asked for without one goes to `/login`; an
//!   API call gets `401`.
//! - A request that changes something and comes with the cookie must come from this site
//!   (its `Origin`, or `Sec-Fetch-Site`), so another site cannot act as someone signed in.
//! - Guessing is slowed down: a pause after each failed sign-in, and after too many from one
//!   address (or for one name) a refusal for a while.
//! - The first account needs the setup code the cloud prints at start while nobody has one,
//!   so whoever reaches a fresh install first on the network cannot take it.

use crate::accounts::{self, Signed};
use crate::store::now;
use crate::{STYLE, Shared, escape, failed};
use axum::{
    Extension, Form, Json, Router,
    extract::{ConnectInfo, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::net::SocketAddr;

pub const COOKIE: &str = "mc_session";
/// Where an API token from the address (`/t/<token>/…`) is put for the guard: set only by
/// [`token_paths`], which removes any that came with the request.
pub const TOKEN_HEADER: &str = "x-maker-connect-token";
/// Failed sign-ins counted over this many seconds.
const WINDOW: i64 = 15 * 60;
/// At most this many failures from one address in the window, then a refusal.
const PER_ADDRESS: usize = 10;
/// At most this many failures for one name in the window, from anywhere.
const PER_NAME: usize = 30;
/// The pause after a failure, so guesses are slow even below the limits.
const PAUSE: std::time::Duration = std::time::Duration::from_millis(600);

/// The address a request came from, as the guard saw it (for logs and limits).
#[derive(Clone, Debug)]
pub struct Client(pub String);

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/login", get(login_page).post(login))
        .route("/setup", get(setup_page).post(setup))
        .route("/logout", post(logout))
        .route("/api/me", get(me))
}

/// Routes that machines use with their own tokens, and the pages to sign in.
pub fn public(path: &str) -> bool {
    matches!(path, "/login" | "/setup" | "/logo.svg")
        || path.starts_with("/invite/")
        || path == "/api/apps/v1/info"
        || path.starts_with("/api/agent/v1/")
        || path.starts_with("/p/")
        || path.starts_with("/f/")
        || path.starts_with("/simplyprint/")
}

/// The guard, in front of every route.
pub async fn guard(State(store): State<Shared>, mut request: Request, next: Next) -> Response {
    let client = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(|| "unknown".to_owned(), |c| c.0.ip().to_string());
    request.extensions_mut().insert(Client(client));
    let path = request.uri().path().to_owned();
    // Programs may call the apps API from a page of another site (LaserFoundry's web
    // version): with a token, never a cookie. The browser's question first, then the answer.
    if path.starts_with(crate::apps::PREFIX) {
        if request.method() == Method::OPTIONS {
            return cors(StatusCode::NO_CONTENT.into_response());
        }
        let mut request = request;
        if request.headers().contains_key(header::ORIGIN) {
            // From another site: its cookie does not count here, only a token.
            request.headers_mut().remove(header::COOKIE);
        }
        return cors(Box::pin(apps_guard(store, request, next)).await);
    }
    if public(&path) {
        return next.run(request).await;
    }
    // An API token: from the address (Moonraker, the OctoPrint plugin), or a header.
    let api_token = request
        .headers()
        .get(TOKEN_HEADER)
        .or_else(|| request.headers().get(header::AUTHORIZATION))
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v).trim().to_owned())
        .filter(|t| t.starts_with(crate::access::TOKEN_PREFIX));
    if let Some(token) = api_token {
        let access = match store.lock().unwrap().token_access(&token) {
            Ok(access) => access,
            Err(error) => return failed(error),
        };
        let Some(access) = access else {
            return (
                StatusCode::UNAUTHORIZED,
                "that API token is not (or no longer) valid",
            )
                .into_response();
        };
        // Not a browser's cookie: another site cannot send it without knowing it.
        request.extensions_mut().insert(access);
        return next.run(request).await;
    }
    let token = cookie(request.headers(), COOKIE);
    let (signed, access, has_users) = {
        let store = store.lock().unwrap();
        let signed = match token.as_deref().map(|t| store.session(t)) {
            Some(Ok(signed)) => signed,
            Some(Err(error)) => return failed(error),
            None => None,
        };
        let access = match signed.as_ref().map(|s| session_access(&store, s)) {
            Some(Ok(access)) => access,
            Some(Err(error)) => return failed(error),
            None => None,
        };
        (signed, access, store.has_users().unwrap_or(true))
    };
    let Some(signed) = signed else {
        return refuse(&request, has_users);
    };
    if !from_this_site(request.method(), request.headers()) {
        return (
            StatusCode::FORBIDDEN,
            "refused: this came from another site",
        )
            .into_response();
    }
    // Signed in, but in no space (removed from the last one): a page says how to get in one.
    if access.is_none() && wants_page(&request) && !matches!(path.as_str(), "/welcome" | "/logout")
    {
        return Redirect::to("/welcome").into_response();
    }
    request.extensions_mut().insert(signed);
    if let Some(access) = access {
        request.extensions_mut().insert(access);
    }
    next.run(request).await
}

/// The rest of the guard, for the apps API.
async fn apps_guard(store: Shared, request: Request, next: Next) -> Response {
    if public(request.uri().path()) {
        return next.run(request).await;
    }
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_owned());
    let access = match token.map(|t| store.lock().unwrap().token_access(&t)) {
        Some(Ok(access)) => access,
        Some(Err(error)) => return failed(error),
        None => None,
    };
    let Some(access) = access else {
        return (
            StatusCode::UNAUTHORIZED,
            "an API token of the space, as Authorization: Bearer mct_… (Space page → API tokens)",
        )
            .into_response();
    };
    let mut request = request;
    request.extensions_mut().insert(access);
    next.run(request).await
}

/// Any site may call the apps API: what it gets depends on the token it sends.
fn cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("authorization, content-type"),
    );
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert("access-control-max-age", HeaderValue::from_static("600"));
    response
}

/// The space a session is in, with the role there: the one it was in, else the first the
/// user belongs to (and the session moves there); `None` when they belong to none.
fn session_access(
    store: &crate::store::Store,
    signed: &Signed,
) -> rusqlite::Result<Option<crate::access::Access>> {
    let mut space = signed.space_id;
    let mut role = match space {
        Some(space) => store.role_of(signed.user_id, space)?,
        None => None,
    };
    if role.is_none() {
        let Some(first) = store.spaces_of(signed.user_id)?.into_iter().next() else {
            return Ok(None);
        };
        store.set_session_space(signed.session_id, first.id)?;
        space = Some(first.id);
        role = Some(first.role);
    }
    let (Some(space), Some(role)) = (space, role) else {
        return Ok(None);
    };
    Ok(Some(crate::access::Access {
        user_id: signed.user_id,
        actor: signed.display_name.clone(),
        space,
        role,
        token_id: None,
        session_id: Some(signed.session_id),
        instance_admin: signed.instance_admin,
    }))
}

fn wants_page(request: &Request) -> bool {
    request.method() == Method::GET
        && request
            .headers()
            .get(header::ACCEPT)
            .and_then(|a| a.to_str().ok())
            .is_some_and(|a| a.contains("text/html"))
}

/// Wraps the whole app: an address `/t/<token>/rest` is served as `/rest` with the token for
/// the guard, so a client that cannot send headers (Moonraker's `[spoolman] server`) still
/// signs in. Runs before routing (a layer on the router would run after it).
pub fn token_paths(inner: Router) -> Router {
    use tower::Layer;
    let rewrite = axum::middleware::from_fn(|mut request: Request, next: Next| async move {
        request.headers_mut().remove(TOKEN_HEADER);
        let path = request.uri().path().to_owned();
        if let Some(rest) = path.strip_prefix("/t/") {
            let (token, after) = rest.split_once('/').unwrap_or((rest, ""));
            if let Ok(value) = HeaderValue::from_str(token) {
                request.headers_mut().insert(TOKEN_HEADER, value);
            }
            let query = request
                .uri()
                .query()
                .map_or(String::new(), |q| format!("?{q}"));
            if let Ok(uri) = format!("/{after}{query}").parse() {
                *request.uri_mut() = uri;
            }
        }
        next.run(request).await
    });
    Router::new().fallback_service(rewrite.layer(inner))
}

/// No session: a page goes to sign in (or to set up, on a fresh install); the rest gets 401.
fn refuse(request: &Request, has_users: bool) -> Response {
    if !wants_page(request) {
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    }
    if !has_users {
        return Redirect::to("/setup").into_response();
    }
    let here = request.uri().path_and_query().map_or("/", |p| p.as_str());
    let to = if here == "/" {
        "/login".to_owned()
    } else {
        format!("/login?next={}", encode(here))
    };
    Redirect::to(&to).into_response()
}

/// A cookie's value from the request.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_owned())
        })
}

/// Whether a request may change something for someone signed in with a cookie: reading is
/// always fine; a change must come from a page of this site. A browser says where a request
/// comes from (`Origin`, `Sec-Fetch-Site`); a client that says nothing is no browser being
/// misused, so it may.
pub fn from_this_site(method: &Method, headers: &HeaderMap) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
        let origin_host = origin
            .to_str()
            .ok()
            .and_then(|o| o.split_once("://"))
            .map(|(_, rest)| rest.trim_end_matches('/'));
        return host.is_some() && origin_host == host;
    }
    !headers
        .get("sec-fetch-site")
        .is_some_and(|s| s.as_bytes() == b"cross-site" || s.as_bytes() == b"same-site")
}

/// Percent-encodes a value for a query string.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Where to go after signing in: only a path on this site, never another site.
fn next_path(next: Option<&str>) -> String {
    match next {
        Some(path)
            if path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains(['\\', '\r', '\n'])
                && !path.starts_with("/login")
                && !path.starts_with("/setup") =>
        {
            path.to_owned()
        }
        _ => "/".to_owned(),
    }
}

/// The session cookie (for `https`, also `Secure`); `None` ends it.
pub(crate) fn session_cookie(token: Option<&str>, headers: &HeaderMap) -> HeaderValue {
    let secure = headers
        .get("x-forwarded-proto")
        .is_some_and(|p| p.as_bytes() == b"https");
    let value = match token {
        Some(token) => format!(
            "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
            accounts::SESSION_SECONDS,
            if secure { "; Secure" } else { "" }
        ),
        None => format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    };
    HeaderValue::from_str(&value).expect("a cookie of plain characters")
}

/// Signed in: the cookie, then on to `to`.
pub(crate) fn signed_in(token: &str, headers: &HeaderMap, to: &str) -> Response {
    let mut response = Redirect::to(to).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(Some(token), headers));
    response
}

/// Whether `key` failed too often lately; forgets failures older than the window.
fn blocked(
    failures: &mut std::collections::HashMap<String, Vec<i64>>,
    key: &str,
    limit: usize,
) -> bool {
    let since = now() - WINDOW;
    let Some(times) = failures.get_mut(key) else {
        return false;
    };
    times.retain(|t| *t > since);
    times.len() >= limit
}

fn failed_once(failures: &mut std::collections::HashMap<String, Vec<i64>>, keys: &[String]) {
    // Without a bound, made-up names could fill the memory: drop what has run out first.
    if failures.len() > 10_000 {
        let since = now() - WINDOW;
        failures.retain(|_, times| times.iter().any(|t| *t > since));
    }
    for key in keys {
        failures.entry(key.clone()).or_default().push(now());
    }
}

#[derive(Deserialize)]
struct NextQuery {
    next: Option<String>,
}

#[derive(Deserialize)]
struct LoginForm {
    name: String,
    password: String,
    next: Option<String>,
}

async fn login_page(State(store): State<Shared>, Query(query): Query<NextQuery>) -> Response {
    if !store.lock().unwrap().has_users().unwrap_or(true) {
        return Redirect::to("/setup").into_response();
    }
    Html(login_html("", "", query.next.as_deref())).into_response()
}

async fn login(
    State(store): State<Shared>,
    Extension(Client(client)): Extension<Client>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    if !from_this_site(&Method::POST, &headers) {
        return (
            StatusCode::FORBIDDEN,
            "refused: this came from another site",
        )
            .into_response();
    }
    let name = form.name.trim().to_owned();
    let keys = [
        format!("ip:{client}"),
        format!("name:{}", name.to_lowercase()),
    ];
    let credentials = {
        let mut store = store.lock().unwrap();
        if blocked(&mut store.failures, &keys[0], PER_ADDRESS)
            || blocked(&mut store.failures, &keys[1], PER_NAME)
        {
            println!("sign-in refused for a while: too many failures ({client}, {name:?})");
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Html(login_html(
                    &name,
                    "Too many failed sign-ins. Try again in a quarter of an hour.",
                    form.next.as_deref(),
                )),
            )
                .into_response();
        }
        match store.credentials(&name) {
            Ok(credentials) => credentials,
            Err(error) => return failed(error),
        }
    };
    // The hash is checked outside the lock: it is slow on purpose. A name that does not
    // exist is checked against a stand-in, so the answer takes as long either way.
    let password = form.password;
    let hash = credentials
        .as_ref()
        .map_or_else(|| stand_in_hash().to_owned(), |c| c.hash.clone());
    let right = tokio::task::spawn_blocking(move || accounts::verify_password(&password, &hash))
        .await
        .unwrap_or(false);
    let user = credentials.filter(|c| right && !c.disabled);
    let Some(user) = user else {
        failed_once(&mut store.lock().unwrap().failures, &keys);
        println!("sign-in failed from {client} for {name:?}");
        tokio::time::sleep(PAUSE).await;
        return (
            StatusCode::UNAUTHORIZED,
            Html(login_html(
                &name,
                "That name and password do not match.",
                form.next.as_deref(),
            )),
        )
            .into_response();
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = {
        let mut store = store.lock().unwrap();
        store.failures.remove(&keys[1]);
        let space = store.first_space_of(user.user_id).ok().flatten();
        match store.create_session(user.user_id, space, user_agent, &client) {
            Ok(token) => token,
            Err(error) => return failed(error),
        }
    };
    signed_in(&token, &headers, &next_path(form.next.as_deref()))
}

/// A hash no password matches, to check unknown names against.
fn stand_in_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| accounts::hash_password(&accounts::random_token()))
}

async fn logout(
    State(store): State<Shared>,
    Extension(signed): Extension<Signed>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = store.lock().unwrap().end_session(signed.session_id) {
        return failed(error);
    }
    let mut response = Redirect::to("/login").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(None, &headers));
    response
}

/// Who is signed in, the space they are in with their role, and the spaces they could
/// go to: for the sidebar.
async fn me(
    State(store): State<Shared>,
    Extension(signed): Extension<Signed>,
    access: Option<Extension<crate::access::Access>>,
) -> Response {
    let store = store.lock().unwrap();
    let spaces = match store.spaces_of(signed.user_id) {
        Ok(spaces) => spaces,
        Err(error) => return failed(error),
    };
    let space = access.as_ref().map(|Extension(a)| {
        serde_json::json!({
            "id": a.space,
            "name": store.space_name(a.space).unwrap_or_default(),
            "role": a.role,
        })
    });
    Json(serde_json::json!({
        "name": signed.name,
        "display_name": signed.display_name,
        "instance_admin": signed.instance_admin,
        "space": space,
        "spaces": spaces,
        "may_make_spaces": signed.instance_admin || store.open_spaces(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct SetupForm {
    code: String,
    space: String,
    display_name: String,
    name: String,
    password: String,
    again: String,
}

async fn setup_page(State(store): State<Shared>) -> Response {
    let store = store.lock().unwrap();
    if store.setup_code.is_none() {
        return Redirect::to("/login").into_response();
    }
    let space = store.default_space().map(|(_, n)| n).unwrap_or_default();
    Html(setup_html(&space, "", "", "")).into_response()
}

/// The first account: an instance admin who owns the first space (and names it).
async fn setup(
    State(store): State<Shared>,
    Extension(Client(client)): Extension<Client>,
    headers: HeaderMap,
    Form(form): Form<SetupForm>,
) -> Response {
    if !from_this_site(&Method::POST, &headers) {
        return (
            StatusCode::FORBIDDEN,
            "refused: this came from another site",
        )
            .into_response();
    }
    let key = format!("setup:{client}");
    let (space_name, display_name, name) = (
        form.space.trim().to_owned(),
        form.display_name.trim().to_owned(),
        form.name.trim().to_owned(),
    );
    let again = |problem: &str| {
        Html(setup_html(&space_name, &display_name, &name, problem)).into_response()
    };
    let wrong_code = {
        let mut store = store.lock().unwrap();
        let Some(code) = store.setup_code.clone() else {
            return Redirect::to("/login").into_response();
        };
        if blocked(&mut store.failures, &key, PER_ADDRESS) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                again("Too many wrong codes. Try again in a quarter of an hour."),
            )
                .into_response();
        }
        let typed = form.code.trim().to_ascii_uppercase().replace(' ', "");
        let typed = if typed.len() == 8 {
            format!("{}-{}", &typed[..4], &typed[4..])
        } else {
            typed
        };
        let wrong = !same(typed.as_bytes(), code.as_bytes());
        if wrong {
            failed_once(&mut store.failures, std::slice::from_ref(&key));
            println!("setup code wrong from {client}");
        }
        wrong
    };
    if wrong_code {
        tokio::time::sleep(PAUSE).await;
        return (
            StatusCode::UNAUTHORIZED,
            again("That is not the setup code. It is in the cloud's log, printed when it started."),
        )
            .into_response();
    }
    let problem = if space_name.is_empty() || space_name.chars().count() > 60 {
        Some("A space needs a name of at most 60 characters.".to_owned())
    } else if display_name.is_empty() || display_name.chars().count() > 60 {
        Some("Your name needs 1 to 60 characters.".to_owned())
    } else if let Some(problem) = accounts::name_problem(&name) {
        Some(format!("Not that sign-in name: {problem}."))
    } else if let Some(problem) = accounts::password_problem(&form.password) {
        Some(format!("Not that password: {problem}."))
    } else if form.password != form.again {
        Some("The two passwords differ.".to_owned())
    } else {
        None
    };
    if let Some(problem) = problem {
        return (StatusCode::BAD_REQUEST, again(&problem)).into_response();
    }
    let password = form.password;
    let hash = tokio::task::spawn_blocking(move || accounts::hash_password(&password))
        .await
        .unwrap_or_default();
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut store = store.lock().unwrap();
    // Two at once: only the first gets it.
    if store.setup_code.is_none() {
        return Redirect::to("/login").into_response();
    }
    let made = (|| {
        let (space, _) = store.default_space()?;
        let user = store.create_user(&name, &display_name, &hash, true)?;
        // Owner of the first space's organisation (all its spaces); both get the name.
        let organisation = store.organisation_of(space)?;
        store.set_org_membership(user, organisation, "owner")?;
        store.rename_space(space, &space_name)?;
        store.rename_organisation(organisation, &space_name)?;
        store.create_session(user, Some(space), user_agent, &client)
    })();
    match made {
        Ok(token) => {
            store.setup_code = None;
            println!("first account made: {name} (instance admin, owner of {space_name:?})");
            signed_in(&token, &headers, "/")
        }
        Err(error) => failed(error),
    }
}

/// Compares secrets in a time that does not depend on where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

const AUTH_STYLE: &str = r#"
  body { display: grid; place-items: center; min-height: 100vh; margin: 0; padding: 0; background: var(--bg); color: var(--ink); }
  .auth form { display: block; margin: 0; }
  .auth { width: min(380px, calc(100vw - 32px)); background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 28px; box-sizing: border-box; }
  .auth .mark { display: flex; align-items: center; gap: 10px; margin-bottom: 18px; }
  .auth h1 { font-size: 1.25rem; margin: 0; }
  .auth p { color: var(--muted); margin: 0 0 16px; line-height: 1.45; }
  .auth label { display: block; font-size: .85rem; font-weight: 600; margin: 12px 0 4px; }
  .auth input { width: 100%; box-sizing: border-box; padding: 9px 11px; border: 1px solid var(--line); border-radius: 8px; background: var(--bg); color: var(--ink); font: inherit; }
  .auth input:focus { outline: 2px solid var(--accent); outline-offset: 1px; }
  .auth button { width: 100%; margin-top: 20px; padding: 10px; border: 0; border-radius: 8px; background: var(--accent); color: #fff; font: inherit; font-weight: 600; cursor: pointer; }
  .auth .problem { color: var(--bad); background: color-mix(in srgb, var(--bad) 10%, transparent); padding: 8px 11px; border-radius: 8px; margin-bottom: 4px; }
  .auth small { color: var(--muted); font-size: .8rem; }
"#;

pub(crate) fn auth_page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · Maker Connect</title><link rel="icon" href="/logo.svg">
<style>{STYLE}{AUTH_STYLE}</style></head>
<body><main class="auth"><div class="mark"><img src="/logo.svg" alt="" width="32" height="32"><h1>{title}</h1></div>
{body}</main></body></html>"#
    )
}

fn problem_html(problem: &str) -> String {
    if problem.is_empty() {
        String::new()
    } else {
        format!(r#"<p class="problem" role="alert">{}</p>"#, escape(problem))
    }
}

fn login_html(name: &str, problem: &str, next: Option<&str>) -> String {
    let next = next.map_or(String::new(), |n| {
        format!(r#"<input type="hidden" name="next" value="{}">"#, escape(n))
    });
    auth_page(
        "Sign in",
        &format!(
            r#"{problem}<form method="post" action="/login">{next}
<label for="name">Name</label><input id="name" name="name" value="{name}" autocomplete="username" autocapitalize="none" required autofocus>
<label for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" required>
<button>Sign in</button></form>"#,
            problem = problem_html(problem),
            name = escape(name),
        ),
    )
}

fn setup_html(space: &str, display_name: &str, name: &str, problem: &str) -> String {
    auth_page(
        "Set up Maker Connect",
        &format!(
            r#"<p>Make the first account. It runs this server and owns the first space. The setup code is in the cloud's log, printed when it started.</p>
{problem}<form method="post" action="/setup">
<label for="code">Setup code</label><input id="code" name="code" autocomplete="off" autocapitalize="characters" placeholder="ABCD-EFGH" required autofocus>
<label for="space">Name of the space</label><input id="space" name="space" value="{space}" maxlength="60" placeholder="Fablab Noord" required>
<label for="display_name">Your name</label><input id="display_name" name="display_name" value="{display_name}" maxlength="60" autocomplete="name" required>
<label for="name">Sign-in name</label><input id="name" name="name" value="{name}" maxlength="40" autocomplete="username" autocapitalize="none" required>
<small>Letters, digits, dots, dashes or underscores.</small>
<label for="password">Password</label><input id="password" name="password" type="password" minlength="{min}" autocomplete="new-password" required>
<label for="again">Password again</label><input id="again" name="again" type="password" minlength="{min}" autocomplete="new-password" required>
<button>Make the account</button></form>"#,
            problem = problem_html(problem),
            space = escape(space),
            display_name = escape(display_name),
            name = escape(name),
            min = accounts::MIN_PASSWORD,
        ),
    )
}

/// For tests: the app with someone signed in on every request (an owner of the first space).
#[cfg(test)]
pub fn signed_in_app(store: Shared) -> Router {
    let token = {
        let store = store.lock().unwrap();
        let user = store.create_user("tester", "Tester", "x", true).unwrap();
        let space = store.default_space().unwrap().0;
        let organisation = store.organisation_of(space).unwrap();
        store
            .set_org_membership(user, organisation, "owner")
            .unwrap();
        store
            .create_session(user, Some(space), "test", "127.0.0.1")
            .unwrap()
    };
    let cookie = HeaderValue::from_str(&format!("{COOKIE}={token}")).unwrap();
    crate::app(store).layer(axum::middleware::map_request(
        move |mut request: Request| {
            let cookie = cookie.clone();
            async move {
                request.headers_mut().insert(header::COOKIE, cookie);
                request
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn fresh() -> Shared {
        Arc::new(Mutex::new(Store::open(":memory:").unwrap()))
    }

    async fn send(
        app: &Router,
        request: axum::http::Request<Body>,
    ) -> (StatusCode, HeaderMap, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    fn form(path: &str, body: &str) -> axum::http::Request<Body> {
        axum::http::Request::post(path)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("host", "cloud.local")
            .header("origin", "http://cloud.local")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    fn page(path: &str) -> axum::http::Request<Body> {
        axum::http::Request::get(path)
            .header("accept", "text/html")
            .body(Body::empty())
            .unwrap()
    }

    fn session_from(headers: &HeaderMap) -> String {
        let set = headers[header::SET_COOKIE].to_str().unwrap();
        assert!(
            set.contains("HttpOnly") && set.contains("SameSite=Lax"),
            "{set}"
        );
        set.split(';').next().unwrap().to_owned()
    }

    /// Every route in the source, with made-up values for its parameters, needs a session,
    /// unless it is one machines use with their own tokens or a page to sign in. A route
    /// added later is checked without anyone remembering to.
    #[tokio::test]
    async fn every_route_needs_a_session_but_the_public_ones() {
        // Every source file, read when the test runs: a new module cannot be forgotten.
        let sources = crate::access::route_sources();
        let mut paths: Vec<String> = Vec::new();
        for source in &sources {
            // `.route("/path"` or `.route(\n "/path"`, the path being a literal.
            for piece in source.split(".route(").skip(1) {
                let piece = piece.trim_start();
                let Some(rest) = piece.strip_prefix('"') else {
                    continue;
                };
                let Some(path) = rest.split('"').next() else {
                    continue;
                };
                if path.starts_with('/') {
                    paths.push(path.to_owned());
                }
            }
        }
        paths.push(maker_connect_protocol::REGISTER.to_owned());
        paths.push(maker_connect_protocol::TELEMETRY.to_owned());
        assert!(
            paths.len() > 90,
            "found only {} routes: is the parsing still right?",
            paths.len()
        );
        let store = fresh();
        store
            .lock()
            .unwrap()
            .create_user("someone", "Someone", "x", true)
            .unwrap();
        let app = crate::app(store);
        let mut checked = 0;
        for path in &paths {
            let concrete: String = path
                .split('/')
                .map(|part| if part.starts_with('{') { "x1" } else { part })
                .collect::<Vec<_>>()
                .join("/");
            if public(&concrete) {
                continue;
            }
            for method in [
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ] {
                let request = axum::http::Request::builder()
                    .method(method.clone())
                    .uri(&concrete)
                    .body(Body::empty())
                    .unwrap();
                let (status, _, _) = send(&app, request).await;
                assert_eq!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{method} {concrete} (route {path}) without a session"
                );
                checked += 1;
            }
        }
        assert!(checked > 400, "{checked}");
        // A page goes to sign in, and comes back after.
        let (status, headers, _) = send(&app, page("/files?x=1")).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(headers[header::LOCATION], "/login?next=/files%3Fx%3D1");
    }

    /// A fresh install sends pages to setup; setup needs the code the cloud printed, and
    /// then makes an owner who is signed in, and the space gets its name.
    #[tokio::test]
    async fn the_first_account_needs_the_setup_code() {
        let store = fresh();
        let code = store.lock().unwrap().setup_code.clone().unwrap();
        let app = crate::app(store.clone());
        let (status, headers, _) = send(&app, page("/")).await;
        assert_eq!(
            (status, &headers[header::LOCATION]),
            (StatusCode::SEE_OTHER, &HeaderValue::from_static("/setup"))
        );
        let fields = |code: &str| {
            format!(
                "code={code}&space=Fablab+Noord&display_name=Timo&name=timo&password=a+long+password&again=a+long+password"
            )
        };
        let (status, _, body) = send(&app, form("/setup", &fields("WRNG-CODE"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("not the setup code"));
        assert!(!store.lock().unwrap().has_users().unwrap());
        // Typed in lower case, without the dash: still the code.
        let typed = code.to_lowercase().replace('-', "");
        let (status, headers, _) = send(&app, form("/setup", &fields(&typed))).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "made");
        let cookie = session_from(&headers);
        {
            let store = store.lock().unwrap();
            assert!(store.setup_code.is_none());
            assert_eq!(store.default_space().unwrap().1, "Fablab Noord");
        }
        let request = axum::http::Request::get("/api/me")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _, body) = send(&app, request).await;
        assert_eq!(status, StatusCode::OK);
        let me: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (me["name"].as_str(), me["instance_admin"].as_bool()),
            (Some("timo"), Some(true))
        );
        assert_eq!(me["space"]["name"], "Fablab Noord");
        // Setup is over.
        let (status, headers, _) = send(&app, form("/setup", &fields(&code))).await;
        assert_eq!(
            (status, &headers[header::LOCATION]),
            (StatusCode::SEE_OTHER, &HeaderValue::from_static("/login"))
        );
    }

    /// Signing in sets the cookie and goes where the person was going (only on this site);
    /// a wrong password does not; signing out ends the session.
    #[tokio::test]
    async fn signing_in_and_out() {
        let store = fresh();
        {
            let store = store.lock().unwrap();
            let user = store
                .create_user(
                    "ann",
                    "Ann",
                    &accounts::hash_password("ann's password"),
                    false,
                )
                .unwrap();
            store.set_membership(user, 1, "member").unwrap();
        }
        let app = crate::app(store.clone());
        let (status, _, body) = send(&app, form("/login", "name=ann&password=wrong")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("do not match") && body.contains(r#"value="ann""#));
        let (status, _, _) = send(&app, form("/login", "name=nobody&password=wrong")).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "an unknown name answers the same"
        );
        let (status, headers, _) = send(
            &app,
            form("/login", "name=ANN&password=ann%27s+password&next=%2Ffiles"),
        )
        .await;
        assert_eq!(
            (status, &headers[header::LOCATION]),
            (StatusCode::SEE_OTHER, &HeaderValue::from_static("/files"))
        );
        let cookie = session_from(&headers);
        for next in ["https://evil.example", "//evil.example", "/\\evil.example"] {
            let (_, headers, _) = send(
                &app,
                form(
                    "/login",
                    &format!("name=ann&password=ann%27s+password&next={}", encode(next)),
                ),
            )
            .await;
            assert_eq!(headers[header::LOCATION], "/", "{next}");
        }
        let with = |request: axum::http::request::Builder| {
            request
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap()
        };
        let (status, _, _) = send(&app, with(axum::http::Request::get("/api/machines"))).await;
        assert_eq!(status, StatusCode::OK);
        // Another site cannot use the cookie to change anything.
        let foreign = axum::http::Request::post("/logout")
            .header("host", "cloud.local")
            .header("origin", "https://evil.example");
        let (status, _, _) = send(&app, with(foreign)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let cross = axum::http::Request::post("/logout").header("sec-fetch-site", "cross-site");
        assert_eq!(send(&app, with(cross)).await.0, StatusCode::FORBIDDEN);
        let own = axum::http::Request::post("/logout")
            .header("host", "cloud.local")
            .header("origin", "http://cloud.local");
        let (status, headers, _) = send(&app, with(own)).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert!(
            headers[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        let (status, _, _) = send(&app, with(axum::http::Request::get("/api/machines"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "signed out");
    }

    /// Too many failures from one address: refused for a while, even with the right password.
    #[tokio::test]
    async fn guessing_is_stopped() {
        let store = fresh();
        store
            .lock()
            .unwrap()
            .create_user(
                "ann",
                "Ann",
                &accounts::hash_password("ann's password"),
                false,
            )
            .unwrap();
        // Failures as if they had happened (without the pauses).
        {
            let mut store = store.lock().unwrap();
            let keys = ["ip:unknown".to_owned()];
            for _ in 0..PER_ADDRESS {
                failed_once(&mut store.failures, &keys);
            }
        }
        let app = crate::app(store.clone());
        let (status, _, body) =
            send(&app, form("/login", "name=ann&password=ann%27s+password")).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(body.contains("Too many"));
        // Older than the window: forgotten.
        for times in store.lock().unwrap().failures.values_mut() {
            for t in times.iter_mut() {
                *t -= WINDOW + 1;
            }
        }
        let (status, _, _) = send(&app, form("/login", "name=ann&password=ann%27s+password")).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }

    #[test]
    fn requests_from_this_site() {
        let headers = |pairs: &[(&'static str, &'static str)]| {
            let mut map = HeaderMap::new();
            for (k, v) in pairs {
                map.insert(*k, HeaderValue::from_static(v));
            }
            map
        };
        assert!(from_this_site(
            &Method::GET,
            &headers(&[("origin", "https://evil.example")])
        ));
        assert!(from_this_site(
            &Method::POST,
            &headers(&[
                ("host", "pi.local:8460"),
                ("origin", "http://pi.local:8460")
            ])
        ));
        assert!(!from_this_site(
            &Method::POST,
            &headers(&[
                ("host", "pi.local:8460"),
                ("origin", "http://pi.local:8461")
            ])
        ));
        assert!(!from_this_site(
            &Method::POST,
            &headers(&[("host", "pi.local"), ("origin", "null")])
        ));
        assert!(from_this_site(
            &Method::POST,
            &headers(&[("sec-fetch-site", "same-origin")])
        ));
        assert!(
            from_this_site(&Method::POST, &headers(&[])),
            "curl and scripts"
        );
        assert!(!from_this_site(
            &Method::PUT,
            &headers(&[("sec-fetch-site", "cross-site")])
        ));
    }

    #[test]
    fn cookies_are_read() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; mc_session=abc; b=2"),
        );
        assert_eq!(cookie(&headers, COOKIE).as_deref(), Some("abc"));
        assert_eq!(cookie(&headers, "c"), None);
    }
}
