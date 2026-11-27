//! The times on the Space page (wiki Design: Devices and Rights, "Times"): the *Opening hours* card
//! on General (the space's time zone, its weekly hours and its closed days) and the *Times* of each
//! group on Permissions. The rules themselves are in [`crate::access::times`].

use crate::access::Access;
use crate::access::times::{self, ClosedDays, SpaceTimes, Window};
use crate::failed;
use crate::store::Store;
use crate::web::shell::escape;
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/space/time-zone", post(set_zone))
        .route("/space/hours", post(add_hours))
        .route("/space/hours/{id}/remove", post(remove_hours))
        .route("/space/closed", post(add_closed))
        .route("/space/closed/{id}/remove", post(remove_closed))
        .route("/permissions/groups/{group}/times", post(add_group_times))
        .route(
            "/permissions/groups/{group}/times/{id}/remove",
            post(remove_group_times),
        )
}

fn x_form(action: &str, title: &str) -> String {
    format!(
        r#"<form method="post" action="{action}" data-async data-then="reload" data-done="Taken away"><span class="fw"><button class="btn primary" title="{title}" aria-label="{title}">×</button></span></form>"#,
        action = escape(action)
    )
}

/// Windows as chips, each with its × for an admin (`remove` is the action before `/{id}/remove`).
pub(crate) fn window_chips(windows: &[Window], remove: Option<&str>) -> String {
    windows
        .iter()
        .map(|w| {
            format!(
                "<li>{}{}</li>",
                escape(&w.label()),
                remove.map_or(String::new(), |base| x_form(
                    &format!("{base}/{}/remove", w.id),
                    "Take this time away"
                ))
            )
        })
        .collect()
}

/// The form that adds a window: the days ticked, from, to.
pub(crate) fn window_form(action: &str, what: &str) -> String {
    let days: String = times::DAYS
        .iter()
        .enumerate()
        .map(|(i, d)| {
            format!(
                r#"<label class="tday"><input type="checkbox" name="day" value="{i}"{c}> {d}</label>"#,
                c = if i < 5 { " checked" } else { "" }
            )
        })
        .collect();
    format!(
        r#"<form method="post" action="{action}" data-async data-then="reload" data-done="Added" class="row twin">{days}<input type="time" name="from" value="09:00" required aria-label="From"><span class="muted">to</span><input type="time" name="to" value="17:00" required aria-label="To"><span class="fw"><button class="btn small ghost">Add {what}</button></span></form>"#,
        action = escape(action),
        what = escape(what)
    )
}

/// The *Opening hours* card of Space → General.
pub(crate) fn hours_card(t: &SpaceTimes, admin: bool) -> String {
    let zone = if admin {
        let options: String = zone_names()
            .iter()
            .map(|z| {
                format!(
                    r#"<option{sel}>{z}</option>"#,
                    sel = if *z == t.time_zone { " selected" } else { "" },
                    z = escape(z)
                )
            })
            .collect();
        format!(
            r#"<form method="post" action="space/time-zone" data-async data-then="reload" data-done="Saved" class="row"><label class="muted">Time zone <select name="zone" onchange="this.form.requestSubmit()">{options}</select></label></form>"#
        )
    } else {
        format!(
            r#"<p class="muted">Time zone: {}</p>"#,
            escape(&t.time_zone)
        )
    };
    let hours = if t.hours.is_empty() {
        r#"<p class="muted">No hours: the space is open at all times.</p>"#.to_owned()
    } else {
        format!(
            r#"<ul class="pchips">{}</ul>"#,
            window_chips(&t.hours, admin.then_some("space/hours"))
        )
    };
    let closed: String = t.closed.iter().map(|c| closed_line(c, admin)).collect();
    let closed = if closed.is_empty() {
        r#"<p class="muted">No closed days.</p>"#.to_owned()
    } else {
        format!(r#"<ul class="list">{closed}</ul>"#)
    };
    let (add_hours, add_closed) = if admin {
        (
            window_form("space/hours", "hours"),
            r#"<form method="post" action="space/closed" data-async data-then="reload" data-done="Added" class="row twin"><input name="name" required maxlength="60" placeholder="Christmas holidays" aria-label="What"><input type="date" name="first" required aria-label="From"><span class="muted">to</span><input type="date" name="last" aria-label="To (the same day when empty)"><span class="fw"><button class="btn small ghost">Add closed days</button></span></form>"#.to_owned(),
        )
    } else {
        (String::new(), String::new())
    };
    format!(
        r#"<section class="card"><h3>Opening hours</h3><p class="muted">When the space is open. Outside these hours and on closed days a door does not open and a new job does not start; a running job goes on. Owners and admins always may. Groups can have their own times too, under Permissions.</p>{zone}<p class="g-label">Hours</p>{hours}{add_hours}<p class="g-label">Closed days</p>{closed}{add_closed}</section>"#
    )
}

fn closed_line(c: &ClosedDays, admin: bool) -> String {
    let when = if c.first_day == c.last_day {
        c.first_day.to_string()
    } else {
        format!("{} – {}", c.first_day, c.last_day)
    };
    format!(
        r#"<li class="row"><span><b>{name}</b> <span class="muted">{when}</span></span>{x}</li>"#,
        name = escape(&c.name),
        x = if admin {
            x_form(&format!("space/closed/{}/remove", c.id), "Open again")
        } else {
            String::new()
        }
    )
}

/// The *Times* of a group's card on Permissions.
pub(crate) fn group_times(group: i64, windows: &[Window], admin: bool) -> String {
    let base = format!("permissions/groups/{group}/times");
    let chips = if windows.is_empty() {
        r#"<li class="muted">At all times</li>"#.to_owned()
    } else {
        window_chips(windows, admin.then_some(base.as_str()))
    };
    format!(
        r#"<p class="g-label">Times</p><ul class="pchips">{chips}</ul>{}"#,
        if admin {
            format!(
                r#"<details class="gtimes"><summary>+ time</summary>{}</details>"#,
                window_form(&base, "time")
            )
        } else {
            String::new()
        }
    )
}

/// The zone names the hub knows, sorted.
fn zone_names() -> Vec<String> {
    let mut names: Vec<String> = jiff::tz::db().available().map(|n| n.to_string()).collect();
    names.sort();
    names
}

#[derive(serde::Deserialize)]
struct ZoneForm {
    zone: String,
}

async fn set_zone(
    access: Access,
    State(store): State<Store>,
    Form(form): Form<ZoneForm>,
) -> Response {
    match store.set_time_zone(access.space, &form.zone).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::BAD_REQUEST, "no such time zone").into_response(),
        Err(error) => failed(error),
    }
}

/// A window from the form: `day=0..6` for each day ticked, `from` and `to` as "09:00".
#[allow(clippy::result_large_err)]
fn window_of(form: &[(String, String)]) -> Result<(u8, u16, u16), Response> {
    let field = |name: &str| {
        form.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    let days = form
        .iter()
        .filter(|(k, _)| k == "day")
        .filter_map(|(_, v)| v.parse::<u8>().ok().filter(|d| *d < 7))
        .fold(0u8, |acc, d| acc | (1 << d));
    if days == 0 {
        return Err((StatusCode::BAD_REQUEST, "tick at least one day").into_response());
    }
    let (Some(from), Some(to)) = (
        times::parse_hhmm(field("from")),
        times::parse_hhmm(field("to")),
    ) else {
        return Err((StatusCode::BAD_REQUEST, "from and to as 09:00").into_response());
    };
    // "00:00" as the end is midnight at the end of the day.
    let to = if to == 0 { 1440 } else { to };
    if to <= from {
        return Err((StatusCode::BAD_REQUEST, "to must be after from").into_response());
    }
    Ok((days, from, to))
}

async fn add_window(
    store: &Store,
    access: &Access,
    group: Option<i64>,
    form: &[(String, String)],
) -> Response {
    let (days, from, to) = match window_of(form) {
        Ok(w) => w,
        Err(refused) => return refused,
    };
    match store.add_window(access.space, group, days, from, to).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such group").into_response(),
        Err(error) => failed(error),
    }
}

async fn remove_window(store: &Store, access: &Access, group: Option<i64>, id: i64) -> Response {
    match store.remove_window(access.space, group, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such time").into_response(),
        Err(error) => failed(error),
    }
}

async fn add_hours(
    access: Access,
    State(store): State<Store>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    add_window(&store, &access, None, &form).await
}

async fn remove_hours(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    remove_window(&store, &access, None, id).await
}

async fn add_group_times(
    access: Access,
    State(store): State<Store>,
    Path(group): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    add_window(&store, &access, Some(group), &form).await
}

async fn remove_group_times(
    access: Access,
    State(store): State<Store>,
    Path((group, id)): Path<(i64, i64)>,
) -> Response {
    remove_window(&store, &access, Some(group), id).await
}

#[derive(serde::Deserialize)]
struct ClosedForm {
    name: String,
    first: String,
    #[serde(default)]
    last: String,
}

async fn add_closed(
    access: Access,
    State(store): State<Store>,
    Form(form): Form<ClosedForm>,
) -> Response {
    let Ok(first) = form.first.parse::<jiff::civil::Date>() else {
        return (StatusCode::BAD_REQUEST, "a first day as 2026-12-24").into_response();
    };
    let last = if form.last.trim().is_empty() {
        first
    } else {
        match form.last.parse::<jiff::civil::Date>() {
            Ok(last) => last,
            Err(_) => return (StatusCode::BAD_REQUEST, "a last day as 2026-12-26").into_response(),
        }
    };
    match store
        .add_closed_days(access.space, first, last, &form.name)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::BAD_REQUEST,
            "a name, and the last day not before the first",
        )
            .into_response(),
        Err(error) => failed(error),
    }
}

async fn remove_closed(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    match store.remove_closed_days(access.space, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such closed days").into_response(),
        Err(error) => failed(error),
    }
}
