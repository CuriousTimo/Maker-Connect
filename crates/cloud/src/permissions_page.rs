//! Permissions: who may use which machine. Per person, per machine and per group of
//! machines; a machine is open to every member of the space, or needs permission. Shown in
//! the Space page: per person in its Members tab, groups and machines in its Permissions tab.
//! (Not to be mixed up with access: getting in, through doors with locks; that is physical,
//! and a thing of its own.)

use crate::access::{Access, Level, Role};
use crate::space_page::day;
use crate::store::Store;
use crate::{Shared, escape, failed};
use axum::{
    Form, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/permissions/people/{user}", post(set_permissions))
        .route("/machines/{id}/ask", post(ask))
        .route("/permissions/requests/{id}/{answer}", post(answer))
        .route("/permissions/groups/{group}/people/{user}", post(tick))
        .route(
            "/permissions/people/{user}/permission",
            post(give_permission),
        )
        .route("/permissions/groups", post(new_group))
        .route("/permissions/groups/{group}/remove", post(remove_group))
        .route("/permissions/groups/{group}/machines", post(group_machine))
        .route(
            "/permissions/groups/{group}/machines/{machine}/remove",
            post(group_machine_out),
        )
        .route("/permissions/groups/{group}/grants", post(group_grant))
        .route(
            "/permissions/groups/{group}/grants/{user}/remove",
            post(group_ungrant),
        )
        .route("/machines/{id}/access", post(machine_access))
        .route("/machines/{id}/grants", post(grant))
        .route("/machines/{id}/grants/{user}/remove", post(ungrant))
}

/// The Space page's access parts: per member what they may use (for the Members tab), the
/// Groups tab, and the Machines tab.
pub(crate) struct Parts {
    /// The matrix of people and machine groups, for the Members tab.
    pub matrix: String,
    pub groups: String,
    pub machines: String,
}

/// The two levels to choose from, one chosen.
fn level_options(chosen: Level) -> String {
    [
        (Level::Use, "use: jobs, labels"),
        (Level::Operate, "operate: also move, heat, G-code"),
    ]
    .iter()
    .map(|(l, text)| {
        format!(
            r#"<option value="{v}"{sel}>{text}</option>"#,
            v = l.as_str(),
            sel = if *l == chosen { " selected" } else { "" }
        )
    })
    .collect()
}

fn x_form(action: &str, title: &str) -> String {
    format!(
        r#"<form method="post" action="{action}" data-async data-then="reload" data-done="Taken away"><button title="{title}" aria-label="{title}">×</button></form>"#,
        action = escape(action)
    )
}

pub(crate) fn parts(store: &Store, access: &Access) -> rusqlite::Result<Parts> {
    let space = access.space;
    let admin = access.role >= Role::Admin;
    let members = store.members(space)?;
    let machines = store.machines(space)?;
    let groups = store.groups(space)?;
    let mut closed_of = Vec::new();
    let mut machine_grants: Vec<(String, i64, crate::access::Grant)> = Vec::new();
    for machine in &machines {
        let closed = store.induction_required(space, &machine.id)?;
        if closed {
            for g in store.grants(space, &machine.id)? {
                machine_grants.push((machine.id.clone(), g.user_id, g));
            }
        }
        closed_of.push((machine, closed));
    }

    // Who may use what: a matrix, a person per row, a machine group per column (and doors,
    // planned). A tick saves at once; Permissions… per person for the rest (use or operate,
    // until when, single machines). Someone who is no admin sees only their own row.
    let mut matrix_rows = String::new();
    for m in members
        .iter()
        .filter(|m| admin || m.user_id == access.user_id)
    {
        let who = escape(&m.display_name);
        let guest = m.role == Role::Guest;
        let cells: String = groups
            .iter()
            .map(|g| {
                if guest {
                    return r#"<td class="mx"><small class="muted">—</small></td>"#.to_owned();
                }
                let grant = g.grants.iter().find(|gr| gr.user_id == m.user_id);
                format!(
                    r#"<td class="mx"><label title="{who} · {group}"><input type="checkbox" data-mx-user="{user}" data-mx-group="{gid}"{checked}{disabled}><small>{level}</small></label></td>"#,
                    group = escape(&g.name),
                    user = m.user_id,
                    gid = g.id,
                    checked = if grant.is_some() { " checked" } else { "" },
                    disabled = if admin { "" } else { " disabled" },
                    level = grant.map(|gr| gr.level.as_str()).unwrap_or(""),
                )
            })
            .collect();
        // Permissions on single machines, outside the groups: counted, changed in the window.
        let single = machine_grants
            .iter()
            .filter(|(_, u, _)| *u == m.user_id)
            .count();
        let body = if guest {
            format!(
                r#"<p>{who} is a guest: guests look only, whatever they are given.</p><p class="muted">Make them a member (the role, above) to let them use machines.</p>"#
            )
        } else if groups.is_empty() && !closed_of.iter().any(|(_, c)| *c) {
            r#"<p>There are no machine groups yet, and every machine is open to members.</p><p class="muted">Make a group under Permissions, or set a machine to "needs permission".</p>"#.to_owned()
        } else {
            let row = |on: String, label: String, grant: Option<&crate::access::Grant>| {
                format!(
                    r#"<tr><td><label class="pick"><input type="checkbox" name="on" value="{on}"{checked}> {label}</label></td><td><select class="small" name="level:{on}" aria-label="May">{levels}</select></td><td><input class="small" type="date" name="until:{on}" value="{date}" aria-label="Until"></td></tr>"#,
                    checked = if grant.is_some() { " checked" } else { "" },
                    levels = level_options(grant.map_or(Level::Use, |g| g.level)),
                    date = grant
                        .and_then(|g| g.expires_at)
                        .map(day)
                        .unwrap_or_default(),
                )
            };
            let mut rows = String::new();
            for g in &groups {
                rows += &row(
                    format!("g:{}", g.id),
                    format!(
                        "<b>{}</b> <small class=\"muted\">group, {} machine{}</small>",
                        escape(&g.name),
                        g.machines.len(),
                        if g.machines.len() == 1 { "" } else { "s" }
                    ),
                    g.grants.iter().find(|gr| gr.user_id == m.user_id),
                );
            }
            for (machine, closed) in &closed_of {
                if !*closed {
                    continue;
                }
                rows += &row(
                    format!("m:{}", machine.id),
                    escape(&machine.name),
                    machine_grants
                        .iter()
                        .find(|(id, u, _)| id == &machine.id && *u == m.user_id)
                        .map(|(_, _, g)| g),
                );
            }
            format!(
                r#"<form method="post" action="permissions/people/{id}" data-async data-then="reload" data-done="Permissions saved"><table class="list perm-table"><thead><tr><th>May use</th><th>As</th><th>Until</th></tr></thead><tbody>{rows}</tbody></table><p class="muted" style="font-size:13px">Use: jobs and labels. Operate: also move, heat, G-code. No date: no end.</p><div class="dialog-actions"><button type="button" class="ghost" data-close>Cancel</button><button>Save</button></div></form>"#,
                id = m.user_id
            )
        };
        let edit = if admin {
            format!(
                r#"<button type="button" class="small ghost" data-open-perm="{id}">Permissions…</button><dialog class="perm-dialog" data-perm="{id}" aria-label="Permissions of {who}"><div class="dialog-head"><h2>Permissions of {who}</h2><button type="button" class="close" data-close aria-label="Close">✕</button></div><section>{body}</section></dialog>"#,
                id = m.user_id
            )
        } else {
            String::new()
        };
        matrix_rows += &format!(
            r#"<tr><td><b>{who}</b><br><small>{role}</small></td>{cells}<td class="mx"><small class="muted">—</small></td><td class="mx-more">{single}{edit}</td></tr>"#,
            role = m.role.as_str(),
            single = match single {
                0 => String::new(),
                1 => "<small>+ 1 machine</small> ".to_owned(),
                n => format!("<small>+ {n} machines</small> "),
            },
        );
    }
    let group_heads: String = groups
        .iter()
        .map(|g| {
            format!(
                r#"<th class="mx" title="{n} machine{s}">{name}</th>"#,
                name = escape(&g.name),
                n = g.machines.len(),
                s = if g.machines.len() == 1 { "" } else { "s" },
            )
        })
        .collect();
    // What people asked the staff for: give (use, on that machine) or decline.
    let asked = if admin {
        store.requests(space)?
    } else {
        Vec::new()
    };
    let requests = if asked.is_empty() {
        String::new()
    } else {
        let rows: String = asked
            .iter()
            .map(|r| {
                format!(
                    r#"<tr><td><b>{who}</b></td><td>{machine}</td><td><small>{what}</small></td><td class="since"><small>{at}</small></td><td class="mx-more"><form method="post" action="permissions/requests/{id}/give" data-async data-then="reload" data-done="Given"><button class="small">{give}</button></form> <form method="post" action="permissions/requests/{id}/decline" data-async data-then="reload" data-done="Declined"><button class="small ghost">Decline</button></form></td></tr>"#,
                    who = escape(&r.who),
                    machine = escape(&r.machine),
                    what = if r.training { "asks for a training" } else { "asks for permission" },
                    at = day(r.asked_at),
                    id = r.id,
                    give = if r.training { "Trained: give it" } else { "Give" },
                )
            })
            .collect();
        format!(
            r#"<section class="section asked"><h3>Asked for <span class="chip planned">{n}</span></h3><p class="sub">People who may not use a machine yet asked the staff. Give is "use" on that machine; more under Permissions….</p><table class="list"><tbody>{rows}</tbody></table></section>"#,
            n = asked.len()
        )
    };
    let matrix = format!(
        r#"{requests}<section class="section"><h3>Who may use what</h3><p class="sub">A person per row, a machine group per column: a tick gives permission to use every machine in it, at once. Permissions… for the rest: use or operate, until when, single machines. Owners and admins too: managing is not using. Groups are made under Permissions.</p><div class="mx-wrap"><table class="list mx-table"><thead><tr><th>Who</th>{group_heads}{no_groups}<th class="mx">Access <span class="chip planned">Planned</span></th><th></th></tr></thead><tbody>{matrix_rows}</tbody></table></div></section>"#,
        no_groups = if groups.is_empty() {
            r#"<th class="mx"><small class="muted">no machine groups yet</small></th>"#
        } else {
            ""
        },
    );

    // Machine groups: only machines; who may use them is set under Members.
    let mut cards = String::new();
    for g in &groups {
        let devices: String = g
            .machines
            .iter()
            .map(|(id, name)| {
                format!(
                    r#"<li>{}{}</li>"#,
                    escape(name),
                    if admin {
                        x_form(
                            &format!("permissions/groups/{}/machines/{}/remove", g.id, id),
                            "Take out of the group",
                        )
                    } else {
                        String::new()
                    }
                )
            })
            .collect();
        let outside: String = machines
            .iter()
            .filter(|m| !g.machines.iter().any(|(id, _)| id == &m.id))
            .map(|m| {
                format!(
                    r#"<option value="{}">{}</option>"#,
                    escape(&m.id),
                    escape(&m.name)
                )
            })
            .collect();
        let add_device = if admin && !outside.is_empty() {
            format!(
                r#"<li class="add"><form method="post" action="permissions/groups/{}/machines" data-async data-then="reload" data-done="Added to the group"><select name="machine" aria-label="Add a machine to {}" onchange="if (this.value) this.form.requestSubmit()"><option value="">+ machine</option>{outside}</select></form></li>"#,
                g.id,
                escape(&g.name)
            )
        } else {
            String::new()
        };
        cards += &format!(
            r#"<article class="gcard"><header><b>{name}</b>{remove}</header><ul class="pchips">{devices}{add_device}</ul>{who}</article>"#,
            name = escape(&g.name),
            remove = if admin {
                format!(
                    r#"<form method="post" action="permissions/groups/{}/remove" data-async data-then="reload" data-done="Group removed" data-confirm="Remove the group {}? The permissions on it go too; those per machine stay."><button class="small ghost" title="Remove the group">Remove</button></form>"#,
                    g.id,
                    escape(&g.name)
                )
            } else {
                String::new()
            },
            who = if g.grants.is_empty() {
                r#"<p class="gwho">Nobody may use it yet: tick it for someone under Members.</p>"#
                    .to_owned()
            } else {
                format!(
                    r#"<p class="gwho">{} may use it (Members).</p>"#,
                    g.grants
                        .iter()
                        .map(|gr| escape(&gr.display_name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        );
    }
    let new_group = if admin {
        r#"<form method="post" action="permissions/groups" data-async data-then="reload" data-done="Group made" class="gnew"><input name="name" required maxlength="60" placeholder="3D printers" aria-label="Name of a new group"><select name="kind" aria-label="Start with"><option value="">empty</option><option value="all">with all machines</option><option value="fdm">with all 3D printers</option><option value="laser">with all lasers</option><option value="label">with all label printers</option></select><button class="ghost">New group</button></form>"#
    } else {
        ""
    };
    let groups_html = format!(
        r#"<section class="section"><h3>Machine groups</h3><p class="sub">Machines that go together: the 3D printers, the lasers. Who may use a group is ticked under Members; permission on a group is permission on every machine in it, also those added later.</p><div class="gcards">{}</div>{new_group}</section>"#,
        if cards.is_empty() {
            r#"<p class="muted">No groups yet.</p>"#.to_owned()
        } else {
            cards
        }
    );

    // Each machine: open to every member, or only with permission; and who has it.
    let mut rows = String::new();
    for (machine, closed) in &closed_of {
        let in_groups: Vec<String> = groups
            .iter()
            .filter(|g| g.machines.iter().any(|(id, _)| id == &machine.id))
            .map(|g| escape(&g.name))
            .collect();
        let who: Vec<String> = members
            .iter()
            .filter_map(|m| {
                let direct = machine_grants
                    .iter()
                    .find(|(id, u, _)| id == &machine.id && *u == m.user_id)
                    .map(|(_, _, g)| g.level);
                let via = groups
                    .iter()
                    .filter(|g| g.machines.iter().any(|(id, _)| id == &machine.id))
                    .flat_map(|g| {
                        g.grants.iter().filter(|gr| {
                            gr.user_id == m.user_id
                                && gr.expires_at.is_none_or(|t| t > crate::store::now())
                        })
                    })
                    .map(|gr| gr.level)
                    .max();
                direct
                    .max(via)
                    .map(|l| format!("{} · {}", escape(&m.display_name), l.as_str()))
            })
            .collect();
        // Three choices, one always on: open to every member, needs permission, or needs a
        // training (a permission that says someone was shown how).
        let mode = if !*closed {
            0
        } else if machine.training {
            2
        } else {
            1
        };
        let toggle = if admin {
            let choice = |value: u8, text: &str| {
                format!(
                    r#"<label><input type="radio" name="induction" value="{value}"{checked} onchange="this.form.requestSubmit()"> {text}</label>"#,
                    checked = if value == mode { " checked" } else { "" }
                )
            };
            format!(
                r#"<form method="post" action="machines/{id}/access" data-async data-then="reload" data-done="Saved" class="pchoice">{a}{b}{c}</form>"#,
                id = escape(&machine.id),
                a = choice(0, "open to every member"),
                b = choice(1, "needs permission"),
                c = choice(2, "needs training"),
            )
        } else {
            format!(
                "<small>{}</small>",
                ["open to every member", "needs permission", "needs training"][mode as usize]
            )
        };
        rows += &format!(
            r#"<tr><td><b>{name}</b></td><td><small>{groups}</small></td><td>{toggle}</td><td><small>{who}</small></td></tr>"#,
            name = escape(&machine.name),
            groups = if in_groups.is_empty() {
                "—".to_owned()
            } else {
                in_groups.join(", ")
            },
            who = if !admin {
                String::new()
            } else if !*closed {
                "every member".to_owned()
            } else if who.is_empty() {
                "nobody yet".to_owned()
            } else {
                who.join(", ")
            },
        );
    }
    let machines_html = format!(
        r#"<section class="section"><h3>Machines</h3><p class="sub">A new machine needs permission, for everyone: owners and admins manage it, but use it only with permission like anyone else. Whoever adds a machine gets it. Open a machine to let every member use it.</p>{}</section>"#,
        if rows.is_empty() {
            r#"<p class="muted">No machines yet.</p>"#.to_owned()
        } else {
            format!(
                r#"<table class="list"><thead><tr><th>Machine</th><th>Groups</th><th>Access</th><th>Who may use it</th></tr></thead><tbody>{rows}</tbody></table>"#
            )
        }
    );
    Ok(Parts {
        matrix,
        groups: groups_html,
        machines: machines_html,
    })
}

#[derive(Deserialize)]
struct AccessForm {
    induction: Option<String>,
}

async fn machine_access(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<AccessForm>,
) -> Response {
    // "1": needs permission; "2": needs training; "0" (or nothing): open to every member.
    let mode = match form.induction.as_deref() {
        Some("1") => 1,
        Some("2") => 2,
        _ => 0,
    };
    match store
        .lock()
        .unwrap()
        .set_access_mode(access.space, &machine, mode)
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct GrantForm {
    user: i64,
    level: String,
    /// `2026-12-31`: the grant ends at the end of that day (UTC); empty: no end.
    until: Option<String>,
}

async fn grant(
    access: Access,
    State(store): State<Shared>,
    Path(machine): Path<String>,
    Form(form): Form<GrantForm>,
) -> Response {
    let (level, until) = match grant_terms(&form.level, form.until.as_deref()) {
        Ok(terms) => terms,
        Err(response) => return *response,
    };
    let store = store.lock().unwrap();
    if store
        .role_of(form.user, access.space)
        .ok()
        .flatten()
        .is_none()
    {
        return (StatusCode::BAD_REQUEST, "not a member of this space").into_response();
    }
    match store.set_grant(
        access.space,
        &machine,
        form.user,
        level,
        access.user_id,
        until,
    ) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

/// What a permission gives (use or operate) and until when (the end of that day).
fn grant_terms(level: &str, until: Option<&str>) -> Result<(Level, Option<i64>), Box<Response>> {
    let Some(level) = Level::parse(level).filter(|l| matches!(l, Level::Use | Level::Operate))
    else {
        return Err(Box::new(
            (StatusCode::BAD_REQUEST, "use or operate").into_response(),
        ));
    };
    let until = match until.map(str::trim).filter(|d| !d.is_empty()) {
        None => None,
        Some(date) => match end_of_day(date) {
            Some(at) => Some(at),
            None => {
                return Err(Box::new(
                    (StatusCode::BAD_REQUEST, "a date as 2026-12-31").into_response(),
                ));
            }
        },
    };
    Ok((level, until))
}

/// The end of a day (`2026-12-31`, UTC), as Unix seconds.
pub(crate) fn end_of_day(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let (y, m, d): (i64, i64, i64) = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    if parts.next().is_some()
        || !(1..=12).contains(&m)
        || !(1..=31).contains(&d)
        || !(1970..=9999).contains(&y)
    {
        return None;
    }
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + 86_399)
}

async fn ungrant(
    access: Access,
    State(store): State<Shared>,
    Path((machine, user)): Path<(String, i64)>,
) -> Response {
    match store
        .lock()
        .unwrap()
        .remove_grant(access.space, &machine, user)
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such permission").into_response(),
        Err(error) => failed(error),
    }
}

// ---- machine groups ------------------------------------------------------------------------

/// Someone asks the staff for permission (or a training) on a machine they may not use.
async fn ask(access: Access, State(store): State<Shared>, Path(machine): Path<String>) -> Response {
    let store = store.lock().unwrap();
    match store.may(&access, &machine, Level::Use) {
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Ok(Some(true)) => return (StatusCode::CONFLICT, "you may use it already").into_response(),
        Err(error) => return failed(error),
        Ok(Some(false)) => {}
    }
    match store.ask_permission(access.space, access.user_id, &machine) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

/// The staff answer a request: `give` (use, on that machine) or `decline`.
async fn answer(
    access: Access,
    State(store): State<Shared>,
    Path((id, answer)): Path<(i64, String)>,
) -> Response {
    let give = match answer.as_str() {
        "give" => true,
        "decline" => false,
        _ => return (StatusCode::NOT_FOUND, "give or decline").into_response(),
    };
    let store = store.lock().unwrap();
    match store.answer_request(access.space, id, give, access.user_id) {
        Ok(Some(r)) => {
            let what = if r.training {
                "a training"
            } else {
                "permission"
            };
            let _ = store.audit(
                &access,
                if give {
                    "gave what was asked for on"
                } else {
                    "declined what was asked for on"
                },
                &r.machine,
                &format!("{}: {what}", r.who),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "no such request").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct TickForm {
    /// "1": ticked; else not.
    on: String,
}

/// A tick in the matrix: someone may use a group (with "use", or as they had it), or not.
/// Answers the level they have now.
async fn tick(
    access: Access,
    State(store): State<Shared>,
    Path((group, user)): Path<(i64, i64)>,
    Form(form): Form<TickForm>,
) -> Response {
    let store = store.lock().unwrap();
    let Ok(Some(name)) = store.group_name(access.space, group) else {
        return (StatusCode::NOT_FOUND, "no such group").into_response();
    };
    if store
        .role_of(user, access.space)
        .ok()
        .flatten()
        .is_none_or(|r| r == Role::Guest)
    {
        return (
            StatusCode::BAD_REQUEST,
            "guests look only: make them a member first",
        )
            .into_response();
    }
    let (who, _) = store.names_for_log(Some(user), None);
    let had = store
        .groups(access.space)
        .ok()
        .and_then(|gs| gs.into_iter().find(|g| g.id == group))
        .and_then(|g| g.grants.into_iter().find(|gr| gr.user_id == user));
    let done = if form.on == "1" {
        match had {
            Some(gr) => Ok(gr.level),
            None => store
                .set_group_grant(group, user, Level::Use, access.user_id, None)
                .map(|()| Level::Use),
        }
        .map(|level| {
            let _ = store.audit(
                &access,
                "gave permission on the group",
                &name,
                &format!("{who} · {}", level.as_str()),
            );
            level.as_str().to_owned()
        })
    } else {
        store.remove_group_grant(group, user).map(|_| {
            let _ = store.audit(&access, "took permission away on the group", &name, &who);
            String::new()
        })
    };
    match done {
        Ok(level) => level.into_response(),
        Err(error) => failed(error),
    }
}

/// Someone's permissions at once, from the window in Members: every group and every machine
/// that needs permission; ticked (`on`) is given with its level and end, unticked taken away.
async fn set_permissions(
    access: Access,
    State(store): State<Shared>,
    Path(user): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let get = |key: &str| form.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let ticked: Vec<&str> = form
        .iter()
        .filter(|(k, _)| k == "on")
        .map(|(_, v)| v.as_str())
        .collect();
    let store = store.lock().unwrap();
    if store
        .role_of(user, access.space)
        .ok()
        .flatten()
        .is_none_or(|r| r == Role::Guest)
    {
        return (
            StatusCode::BAD_REQUEST,
            "guests look only: make them a member first",
        )
            .into_response();
    }
    let result = (|| -> rusqlite::Result<Result<Vec<String>, Box<Response>>> {
        let mut given = Vec::new();
        for g in store.groups(access.space)? {
            let on = format!("g:{}", g.id);
            if ticked.contains(&on.as_str()) {
                let (level, until) = match grant_terms(
                    get(&format!("level:{on}")).unwrap_or("use"),
                    get(&format!("until:{on}")),
                ) {
                    Ok(t) => t,
                    Err(r) => return Ok(Err(r)),
                };
                store.set_group_grant(g.id, user, level, access.user_id, until)?;
                given.push(format!("{} · {}", g.name, level.as_str()));
            } else {
                store.remove_group_grant(g.id, user)?;
            }
        }
        for machine in store.machines(access.space)? {
            if !store.induction_required(access.space, &machine.id)? {
                continue;
            }
            let on = format!("m:{}", machine.id);
            if ticked.contains(&on.as_str()) {
                let (level, until) = match grant_terms(
                    get(&format!("level:{on}")).unwrap_or("use"),
                    get(&format!("until:{on}")),
                ) {
                    Ok(t) => t,
                    Err(r) => return Ok(Err(r)),
                };
                store.set_grant(
                    access.space,
                    &machine.id,
                    user,
                    level,
                    access.user_id,
                    until,
                )?;
                given.push(format!("{} · {}", machine.name, level.as_str()));
            } else {
                store.remove_grant(access.space, &machine.id, user)?;
            }
        }
        Ok(Ok(given))
    })();
    match result {
        Ok(Ok(given)) => {
            let (who, _) = store.names_for_log(Some(user), None);
            let _ = store.audit(
                &access,
                "set the permissions of",
                &who,
                &if given.is_empty() {
                    "only the open machines".to_owned()
                } else {
                    given.join(", ")
                },
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(response)) => *response,
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct PermissionForm {
    /// `g:<group id>` or `m:<machine id>`.
    on: String,
    level: String,
    until: Option<String>,
}

/// Gives a member permission on a group or a machine, or changes it: from the Members tab.
async fn give_permission(
    access: Access,
    State(store): State<Shared>,
    Path(user): Path<i64>,
    Form(form): Form<PermissionForm>,
) -> Response {
    let (level, until) = match grant_terms(&form.level, form.until.as_deref()) {
        Ok(terms) => terms,
        Err(response) => return *response,
    };
    let store = store.lock().unwrap();
    if store
        .role_of(user, access.space)
        .ok()
        .flatten()
        .is_none_or(|r| r == Role::Guest)
    {
        return (
            StatusCode::BAD_REQUEST,
            "guests look only: make them a member first",
        )
            .into_response();
    }
    let (who, _) = store.names_for_log(Some(user), None);
    let until_text = until
        .map(|t| format!(" until {}", day(t)))
        .unwrap_or_default();
    let done = if let Some(group) = form
        .on
        .strip_prefix("g:")
        .and_then(|g| g.parse::<i64>().ok())
    {
        let Ok(Some(name)) = store.group_name(access.space, group) else {
            return (StatusCode::NOT_FOUND, "no such group").into_response();
        };
        store
            .set_group_grant(group, user, level, access.user_id, until)
            .map(|()| ("gave permission on the group", name))
    } else if let Some(machine) = form.on.strip_prefix("m:") {
        let Ok(Some(m)) = store.machine(access.space, machine) else {
            return (StatusCode::NOT_FOUND, "no such machine").into_response();
        };
        store
            .set_grant(access.space, machine, user, level, access.user_id, until)
            .map(|()| ("gave permission on", m.name))
    } else {
        return (StatusCode::BAD_REQUEST, "a group or a machine").into_response();
    };
    match done {
        Ok((action, target)) => {
            let _ = store.audit(
                &access,
                action,
                &target,
                &format!("{who} · {}{until_text}", level.as_str()),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct GroupForm {
    name: String,
    /// fdm, laser or label: start with all the space's machines of that kind.
    kind: Option<String>,
}

async fn new_group(
    access: Access,
    State(store): State<Shared>,
    Form(form): Form<GroupForm>,
) -> Response {
    let name: String = form.name.trim().chars().take(60).collect();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "a name").into_response();
    }
    let kind = form
        .kind
        .as_deref()
        .filter(|k| matches!(*k, "all" | "fdm" | "laser" | "label"));
    let store = store.lock().unwrap();
    match store.create_group(access.space, &name, kind) {
        Ok(_) => {
            let filled = match kind {
                Some("all") => "with all machines",
                Some("fdm") => "with all 3D printers",
                Some("laser") => "with all lasers",
                Some("label") => "with all label printers",
                _ => "",
            };
            let _ = store.audit(&access, "made the machine group", &name, filled);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => failed(error),
    }
}

async fn remove_group(
    access: Access,
    State(store): State<Shared>,
    Path(group): Path<i64>,
) -> Response {
    let store = store.lock().unwrap();
    let name = store
        .group_name(access.space, group)
        .ok()
        .flatten()
        .unwrap_or_default();
    match store.remove_group(access.space, group) {
        Ok(true) => {
            let _ = store.audit(&access, "removed the machine group", &name, "");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "no such group").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct GroupMachineForm {
    machine: String,
}

async fn group_machine(
    access: Access,
    State(store): State<Shared>,
    Path(group): Path<i64>,
    Form(form): Form<GroupMachineForm>,
) -> Response {
    let store = store.lock().unwrap();
    match store.set_group_machine(access.space, group, &form.machine, true) {
        Ok(true) => {
            let name = store
                .group_name(access.space, group)
                .ok()
                .flatten()
                .unwrap_or_default();
            let (_, machine) = store.names_for_log(None, Some(&form.machine));
            let _ = store.audit(&access, "put a machine in the group", &name, &machine);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "no such group or machine").into_response(),
        Err(error) => failed(error),
    }
}

async fn group_machine_out(
    access: Access,
    State(store): State<Shared>,
    Path((group, machine)): Path<(i64, String)>,
) -> Response {
    let store = store.lock().unwrap();
    match store.set_group_machine(access.space, group, &machine, false) {
        Ok(true) => {
            let name = store
                .group_name(access.space, group)
                .ok()
                .flatten()
                .unwrap_or_default();
            let (_, machine) = store.names_for_log(None, Some(&machine));
            let _ = store.audit(&access, "took a machine out of the group", &name, &machine);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "no such group or machine").into_response(),
        Err(error) => failed(error),
    }
}

async fn group_grant(
    access: Access,
    State(store): State<Shared>,
    Path(group): Path<i64>,
    Form(form): Form<GrantForm>,
) -> Response {
    let (level, until) = match grant_terms(&form.level, form.until.as_deref()) {
        Ok(terms) => terms,
        Err(response) => return *response,
    };
    let store = store.lock().unwrap();
    if !matches!(store.group_name(access.space, group), Ok(Some(_))) {
        return (StatusCode::NOT_FOUND, "no such group").into_response();
    }
    if store
        .role_of(form.user, access.space)
        .ok()
        .flatten()
        .is_none()
    {
        return (StatusCode::BAD_REQUEST, "not a member of this space").into_response();
    }
    match store.set_group_grant(group, form.user, level, access.user_id, until) {
        Ok(()) => {
            let name = store
                .group_name(access.space, group)
                .ok()
                .flatten()
                .unwrap_or_default();
            let (who, _) = store.names_for_log(Some(form.user), None);
            let until = until
                .map(|t| format!(" until {}", day(t)))
                .unwrap_or_default();
            let _ = store.audit(
                &access,
                "gave permission on the group",
                &name,
                &format!("{who} · {}{until}", level.as_str()),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => failed(error),
    }
}

async fn group_ungrant(
    access: Access,
    State(store): State<Shared>,
    Path((group, user)): Path<(i64, i64)>,
) -> Response {
    let store = store.lock().unwrap();
    if !matches!(store.group_name(access.space, group), Ok(Some(_))) {
        return (StatusCode::NOT_FOUND, "no such group").into_response();
    }
    match store.remove_group_grant(group, user) {
        Ok(true) => {
            let name = store
                .group_name(access.space, group)
                .ok()
                .flatten()
                .unwrap_or_default();
            let (who, _) = store.names_for_log(Some(user), None);
            let _ = store.audit(&access, "took permission away on the group", &name, &who);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "no such permission").into_response(),
        Err(error) => failed(error),
    }
}
