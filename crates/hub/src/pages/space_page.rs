//! A space's own page (its members, invite links, who may use which machine, API tokens,
//! the audit log), joining by an invite link, choosing or making a space, and one's own
//! password. What each needs is in [`crate::access::RULES`]; the rules within (no role
//! above one's own, an organisation keeps an owner) are here.

use crate::access::{Access, Role};
use crate::accounts::{self, Signed};
use crate::auth::{Client, from_this_site};
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Extension, Form, Json, Router,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, header as h},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/space", get(page))
        .route("/space/{tab}", get(tab_page))
        .route("/account", get(account_page))
        .route("/api/space/audit", get(audit))
        .route("/space/name", post(rename))
        .route("/organisation/name", post(rename_organisation))
        .route("/space/invites", post(new_invite))
        .route("/space/invites/{id}/revoke", post(revoke_invite))
        .route("/space/members/{user}/role", post(set_role))
        .route("/space/members/{user}/remove", post(remove_member))
        .route("/space/tokens", get(tokens_page).post(new_token))
        .route("/space/tokens/{id}/revoke", post(revoke_token))
        .route("/invite/{token}", get(invite_page).post(join))
        .route("/welcome", get(welcome))
        .route("/spaces/switch", post(switch))
        .route("/spaces", post(new_space))
        .route("/account/password", post(password))
        .route("/instance/open-spaces", post(open_spaces))
}

fn refused(why: &str) -> Response {
    (StatusCode::FORBIDDEN, why.to_owned()).into_response()
}

pub(crate) fn day(at: i64) -> String {
    crate::spoolman::iso(at).chars().take(10).collect()
}

/// An instant shown to a person as a date in their own time zone (#159): the server writes the
/// instant, the browser (`shell.js`) writes the viewer's local date; the UTC date stays as the
/// text until it does.
pub(crate) fn local_day(at: i64) -> String {
    format!(
        r#"<time datetime="{}" data-local="day">{}</time>"#,
        crate::spoolman::iso(at),
        day(at)
    )
}

/// As [`local_day`], with the time to the minute.
pub(crate) fn local_minute(at: i64) -> String {
    let iso = crate::spoolman::iso(at);
    let text: String = iso.replace('T', " ").chars().take(16).collect();
    format!(r#"<time datetime="{iso}" data-local="minute">{text}</time>"#)
}

pub(crate) const SPACE_STYLE: &str = r#"
  .pchips { margin: 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 6px; }
  .pchips li { display: inline-flex; align-items: center; gap: 4px; padding: 2px 4px 2px 10px; border: 1px solid var(--line); border-radius: 0; font-size: 13px; white-space: nowrap; }
  .pchips li.group { border-color: color-mix(in srgb, var(--accent-text) 45%, var(--line)); }
  .pchips form { margin: 0; }
  .pchips button { padding: 0 6px; background: none; color: var(--muted); font-size: 15px; border: 0; }
  .pchips li.add { border-style: dashed; padding: 0; }
  .pchips li.add select { border: 0; background: transparent; font-size: 13px; padding: 3px 8px; color: var(--muted); }
  td.may { min-width: 220px; }
  .give { margin-top: 6px; }
  .give summary { cursor: pointer; color: var(--accent-text); font-size: 13px; font-weight: 600; list-style: none; display: inline-block; }
  .give summary::-webkit-details-marker { display: none; }
  .give form { display: flex !important; flex-wrap: wrap !important; gap: 6px; align-items: center; margin-top: 6px !important; }
  .give label { font-size: 13px; }
  /* A chip that opens to change it, and "+ …" that opens a small form under it. */
  .pchips li { position: relative; }
  .pchips details.edit summary { list-style: none; cursor: pointer; }
  .pchips details.edit summary::-webkit-details-marker { display: none; }
  .pchips li.add details.edit summary { color: var(--accent-text); font-weight: 600; padding: 2px 10px 2px 0; }
  .pchips details.edit[open] > form { position: absolute; top: calc(100% + 6px); left: 0; z-index: 30; display: flex; flex-wrap: wrap; gap: 6px; align-items: center; width: max-content; max-width: min(520px, 80vw); padding: 10px; background: var(--surface); border: 1px solid var(--line); border-radius: 0; box-shadow: 0 10px 30px rgb(0 0 0 / .25); }
  .pchips details.edit form label { font-size: 13px; }
  .gcard .g-label { margin: 0 0 -4px; font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; color: var(--muted); }
  .gcard header { justify-content: space-between; }
  .perm-cell { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  .perm-dialog { width: min(640px, calc(100vw - 32px)); }
  .perm-dialog section { padding: 14px 22px 20px; }
  /* Inside the Members table, but not of it: its own layout. */
  .perm-dialog section > form { display: block !important; }
  .perm-dialog .perm-table .pick { white-space: normal; }
  .perm-dialog p { margin: 10px 0 0; }
  .perm-table td { vertical-align: middle; }
  .perm-table .pick { display: inline-flex; gap: 8px; align-items: center; cursor: pointer; }
  .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 12px; }
  /* Access (planned): the design, marked as such. */
  .chip.planned { background: color-mix(in srgb, var(--st-warn) 18%, transparent); color: var(--st-warn); }
  .a-head { display: flex; align-items: center; gap: 10px; }
  .a-head h3 { margin: 0 0 4px; }
  .a-flow { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: 10px; counter-reset: a; }
  .a-flow li { display: grid; gap: 4px; padding: 12px 14px; border: 1px solid var(--line); border-radius: 0; }
  .a-flow small { color: var(--muted); font-size: 13px; }
  .a-n { display: inline-grid; place-items: center; width: 22px; height: 22px; border-radius: 0; background: var(--line); font-size: 12px; font-weight: 700; }
  .a-form { display: flex; flex-wrap: wrap; gap: 8px; }
  .a-form input { flex: 1 1 220px; }
  .access-plan button:disabled { opacity: .45; cursor: not-allowed; }
  .a-example { opacity: .7; }
  .a-note { margin: 8px 0 0; font-size: 12px; }
  .a-state { font-size: 12px; font-weight: 600; } .a-state.ok { color: var(--accent-text); } .a-state.warn { color: var(--st-warn); }
  .a-log { list-style: none; margin: 0; padding: 0; font-size: 14px; }
  .a-log li { padding: 6px 0; border-top: 1px solid var(--line); } .a-log li:first-child { border-top: 0; }
  .a-log time { color: var(--muted); margin-right: 10px; font-variant-numeric: tabular-nums; }
  .mx-wrap { overflow-x: auto; }
  .mx-table th.mx, .mx-table td.mx { text-align: center; white-space: nowrap; }
  .mx-table td.mx label { display: inline-flex; flex-direction: column; align-items: center; gap: 2px; cursor: pointer; min-width: 48px; }
  .mx-table td.mx input { width: 18px; height: 18px; margin: 0; }
  .mx-table td.mx .mx-level { display: block; margin: 2px auto 0; padding: 0 6px; border: 0; border-radius: 0; background: none; color: var(--muted); font-size: 11px; font-weight: 600; cursor: pointer; text-decoration: underline dotted; }
  .mx-table td.mx .mx-level:hover:not(:disabled) { color: var(--ink); background: var(--line); }
  .mx-table td.mx-more { text-align: right; white-space: nowrap; }
  .gcards { display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 12px; }
  .gcard { border: 1px solid var(--line); border-radius: 0; padding: 12px 14px; display: grid; gap: 10px; align-content: start; }
  .gcard header { display: flex; align-items: baseline; gap: 8px; }
  .gcard header small { color: var(--muted); margin-right: auto; }
  .gcard header form { margin: 0; }
  .gwho { margin: 0; font-size: 13px; color: var(--muted); }
  .gnew { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 12px; }
  .pchoice { display: inline-flex !important; gap: 14px !important; flex-wrap: wrap !important; }
  .pchoice label { display: inline-flex; gap: 6px; align-items: center; font-size: 13px; white-space: nowrap; cursor: pointer; }
  .machine-access { border-top: 1px solid var(--line); padding: 12px 0; }
  .machine-access:first-of-type { border-top: 0; }
  .machine-access header { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
  .machine-access .grants { margin: 8px 0 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 6px; }
  .machine-access .grants li { display: inline-flex; align-items: center; gap: 6px; padding: 3px 4px 3px 10px; border: 1px solid var(--line); border-radius: 0; font-size: 13px; }
  .machine-access .grants form { margin: 0; }
  .machine-access .g-label { margin: 10px 0 0; font-size: 12px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; color: var(--muted); }
  .machine-access header small { margin-right: auto; }
  .machine-access .grants button { padding: 0 6px; background: none; color: var(--muted); font-size: 15px; }
  .audit li { padding: 6px 0; border-bottom: 1px solid var(--line); font-size: 14px; list-style: none; }
  .audit { padding: 0; margin: 0; max-height: 420px; overflow-y: auto; }
  .audit time { color: var(--muted); font-variant-numeric: tabular-nums; margin-right: 8px; }
"#;

/// Space's tabs, one subject each (docs/design/ui.md "Space (admin)"), as pages: a navigation
/// between pages is a list of links with `aria-current`. A member or guest has one: their own
/// permissions, read only ("My permissions").
const TABS: [(&str, &str); 6] = [
    ("general", "General"),
    ("members", "Members"),
    ("permissions", "Permissions"),
    ("access", "Access"),
    ("tokens", "API tokens"),
    ("audit", "Audit log"),
];

async fn page(access: Access, store: State<Store>, signed: Option<Extension<Signed>>) -> Response {
    space_tab(access, store, signed, None, false).await
}

async fn tab_page(
    access: Access,
    store: State<Store>,
    signed: Option<Extension<Signed>>,
    Path(tab): Path<String>,
) -> Response {
    space_tab(access, store, signed, Some(tab), false).await
}

/// API tokens: its path is also where a new one is made (POST).
async fn tokens_page(
    access: Access,
    store: State<Store>,
    signed: Option<Extension<Signed>>,
) -> Response {
    space_tab(access, store, signed, Some("tokens".to_owned()), false).await
}

/// One's own account (the password), from the user menu.
async fn account_page(
    access: Access,
    store: State<Store>,
    signed: Option<Extension<Signed>>,
) -> Response {
    space_tab(access, store, signed, None, true).await
}

async fn space_tab(
    access: Access,
    State(store): State<Store>,
    signed: Option<Extension<Signed>>,
    tab: Option<String>,
    account: bool,
) -> Response {
    // With an API token (no session) the page shows all but one's own account.
    let signed = signed.map(|Extension(s)| s);
    let built: Result<(StatusCode, String), crate::store::BoxError> = async {
        let space = access.space;
        let name = store.space_name(space).await?;
        let org_name = store.organisation_name(space).await?;
        let org_role = store.org_role_of(access.user_id, access.space).await?;
        let admin = access.role >= Role::Admin;
        let org_admin = org_role.is_some_and(|r| r >= Role::Admin);
        let siblings: Vec<_> = store
            .spaces_of(access.user_id)
            .await?
            .into_iter()
            .filter(|s| s.organisation == org_name)
            .collect();
        let roles = |upto: Role, chosen: Option<Role>, none: bool| {
            let mut out = String::new();
            if none {
                out += r#"<option value="none">—</option>"#;
            }
            for role in Role::ALL.iter().rev().filter(|r| **r <= upto) {
                out += &format!(
                    r#"<option value="{r}"{sel}>{r}</option>"#,
                    r = role.as_str(),
                    sel = if chosen == Some(*role) {
                        " selected"
                    } else {
                        ""
                    }
                );
            }
            out
        };

        // Members, with what each may use (crate::permissions_page).
        let members = store.members(space).await?;
        let access_parts = crate::permissions_page::parts(&store, &access).await?;
        let mut rows = String::new();
        for m in &members {
            let you = m.user_id == access.user_id;
            let editable = admin && m.role <= access.role;
            let layer = |label: &str, scope: &str, have: Option<Role>, allowed: bool| {
                if editable && allowed {
                    format!(
                        r#"<form method="post" action="space/members/{id}/role" data-async data-then="reload" data-done="Role changed"><input type="hidden" name="scope" value="{scope}"><select name="role" aria-label="Role {label}" onchange="this.form.requestSubmit()">{opts}</select></form>"#,
                        id = m.user_id,
                        opts = roles(access.role, have, true),
                    )
                } else {
                    have.map_or("<small>—</small>".to_owned(), |r| {
                        format!(r#"<span class="tag">{r}</span>"#, r = r.as_str())
                    })
                }
            };
            rows += &format!(
                r#"<tr><td><b>{display}</b>{you}<br><small>{name}</small></td><td>{org}</td><td>{own}</td><td class="since"><small>{since}</small></td><td>{remove}</td></tr>"#,
                display = escape(&m.display_name),
                you = if you { r#" <small>(you)</small>"# } else { "" },
                name = escape(&m.name),
                org = layer("in the organisation", "organisation", m.org_role, org_admin),
                own = layer("in this space", "space", m.space_role, true),
                since = local_day(m.since),
                remove = if editable && !you {
                    format!(
                        r#"<form method="post" action="space/members/{id}/remove" data-async data-then="reload" data-done="Removed" data-confirm="Take {who} out of {what}?"><input type="hidden" name="scope" value="{scope}"><span class="fw"><button class="btn small danger">Remove</button></span></form>"#,
                        id = m.user_id,
                        who = escape(&m.display_name),
                        what = if m.org_role.is_some() {
                            escape(&org_name)
                        } else {
                            escape(&name)
                        },
                        scope = if m.org_role.is_some() {
                            "organisation"
                        } else {
                            "space"
                        },
                    )
                } else {
                    String::new()
                },
            );
        }
        let members_html = format!(
            r#"<section class="card"><h3>Members</h3><p class="muted">A role in the organisation counts in all of its spaces; a role in this space only here. The higher one counts. Owners and admins manage the space and every machine; using a machine takes permission, for everyone (below), unless it is open; guests look only.</p>
<div class="table-wrap"><table><thead><tr><th>Who</th><th>Organisation</th><th>This space</th><th>Since</th><th></th></tr></thead><tbody>{rows}</tbody></table></div></section>"#
        );

        // Invite links.
        let invites_html = if admin {
            let mut list = String::new();
            for i in store.invites(space).await?.iter().filter(|i| i.open()).take(20) {
                list += &format!(
                    r#"<tr><td><span class="tag">{r}</span></td><td>{scope}</td><td><small>{by}, until {until}</small></td><td><form method="post" action="space/invites/{id}/revoke" data-async data-then="reload" data-done="Revoked"><span class="fw"><button class="btn small ghost">Revoke</button></span></form></td></tr>"#,
                    r = i.role.as_str(),
                    scope = if i.whole_organisation {
                        escape(&org_name)
                    } else {
                        escape(&name)
                    },
                    by = escape(&i.by),
                    until = local_day(i.expires_at),
                    id = i.id,
                );
            }
            let scope_choice = if org_admin {
                format!(
                    r#"<select name="scope" aria-label="Into"><option value="organisation">all of {org}</option><option value="space">{space} only</option></select>"#,
                    org = escape(&org_name),
                    space = escape(&name)
                )
            } else {
                r#"<input type="hidden" name="scope" value="space">"#.to_owned()
            };
            format!(
                r#"<section class="card"><h3>Invite links</h3><p class="muted">A link for one person, valid a week: they make their account (or sign in) and are in. No email needed: send it however you like.</p>
<form method="post" action="space/invites" data-async data-done="Link made" data-secret="invite" class="row"><label>As <select name="role">{roles}</select></label><label>Into {scope_choice}</label><span class="fw"><button class="btn primary">Make a link</button></span></form>
<div data-secret-box="invite" hidden class="secret"><b>The link, shown once:</b><pre data-secret-text></pre><span class="fw"><button type="button" class="btn small ghost" data-copy-secret>Copy</button></span></div>
{table}</section>"#,
                roles = roles(access.role, Some(Role::Member), false),
                table = if list.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<div class="table-wrap" style="margin-top:12px"><table><thead><tr><th>Role</th><th>Into</th><th>Made</th><th></th></tr></thead><tbody>{list}</tbody></table></div>"#
                    )
                },
            )
        } else {
            String::new()
        };

        // API tokens.
        // The cameras to manage (crate::cameras), for an admin, when the hub has them.
        let space_machines = store.machines(space).await?;
        let cameras_html = crate::cameras_space_tab(&store, &access, &space_machines).await;
        let tokens_html = if admin {
            let mut list = String::new();
            for t in store.tokens(space).await? {
                list += &format!(
                    r#"<tr><td><b>{name}</b></td><td><span class="tag">{r}</span></td><td><small>{by}, {made}</small></td><td><small>{used}</small></td><td><form method="post" action="space/tokens/{id}/revoke" data-async data-then="reload" data-done="Revoked" data-confirm="Revoke {name}? What uses it stops working."><span class="fw"><button class="btn small danger">Revoke</button></span></form></td></tr>"#,
                    name = escape(&t.name),
                    r = t.role.as_str(),
                    by = escape(&t.by),
                    made = local_day(t.created_at),
                    used = t
                        .last_used
                        .map_or("never used".to_owned(), |u| format!("used {}", local_day(u))),
                    id = t.id,
                );
            }
            format!(
                r#"<section class="card"><h3>API tokens</h3><p class="muted">For programs: Moonraker's and OctoPrint's Spoolman (they take the address with the token in it), LaserFoundry, scripts (<code>Authorization: Bearer</code>). A token acts in this space with its role, never more than yours.</p>
<form method="post" action="space/tokens" data-async data-done="Token made" data-secret="token" class="row"><input name="name" required maxlength="60" placeholder="Voron's Moonraker" aria-label="Name"><select name="role" aria-label="Role">{roles}</select><span class="fw"><button class="btn primary">Make a token</button></span></form>
<div data-secret-box="token" hidden class="secret"><b>The token, shown once:</b><pre data-secret-text></pre><span class="muted">For Moonraker, in <code>moonraker.conf</code>:</span><pre data-secret-moonraker></pre><span class="fw"><button type="button" class="btn small ghost" data-copy-secret>Copy the token</button></span></div>
{table}</section>"#,
                roles = roles(access.role.min(Role::Admin), Some(Role::Member), false),
                table = if list.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<div class="table-wrap" style="margin-top:12px"><table><thead><tr><th>Name</th><th>Role</th><th>Made</th><th>Used</th><th></th></tr></thead><tbody>{list}</tbody></table></div>"#
                    )
                },
            )
        } else {
            String::new()
        };

        // The audit log.
        let audit_html = if admin {
            let mut lines = String::new();
            for line in store.audit_log(space, 150).await? {
                lines += &format!(
                    r#"<li>{at}<b>{actor}</b> {action} {target}{detail}</li>"#,
                    at = local_minute(line.at),
                    actor = escape(&line.actor),
                    action = escape(&line.action),
                    target = escape(&line.target),
                    detail = if line.detail.is_empty() {
                        String::new()
                    } else {
                        format!(r#" <small class="muted">{}</small>"#, escape(&line.detail))
                    },
                );
            }
            format!(
                r#"<section class="card"><h3>Audit log</h3><p class="muted">Who changed what in this space, newest first (times in UTC).</p><ul class="audit">{}</ul></section>"#,
                if lines.is_empty() {
                    r#"<li class="muted">Nothing yet.</li>"#.to_owned()
                } else {
                    lines
                }
            )
        } else {
            String::new()
        };

        // The organisation and its spaces.
        let switch_list: String = siblings
            .iter()
            .map(|s| {
                if s.id == space {
                    format!(r#"<span class="tag">{}</span>"#, escape(&s.name))
                } else {
                    format!(
                        r#"<form method="post" action="spaces/switch" data-async data-then="./"><input type="hidden" name="space" value="{id}"><span class="fw"><button class="btn small ghost">{name}</button></span></form>"#,
                        id = s.id,
                        name = escape(&s.name)
                    )
                }
            })
            .collect();
        let new_space = if org_admin {
            format!(
                r#"<form method="post" action="spaces" data-async data-then="./" data-done="Space made" class="row"><input type="hidden" name="organisation" value="this"><input name="name" required maxlength="60" placeholder="A second location" aria-label="Name of the new space"><span class="fw"><button class="btn small">New space in {org}</button></span></form>"#,
                org = escape(&org_name)
            )
        } else {
            String::new()
        };
        // Two names: the organisation's (who it is), and this space's (which place).
        let name_form = if admin {
            let org_form = if org_admin {
                format!(
                    r#"<form method="post" action="organisation/name" data-async data-then="reload" data-done="Renamed" class="row"><label class="muted">Organisation <input name="name" value="{org}" required maxlength="60"></label><span class="fw"><button class="btn small ghost">Rename</button></span></form>"#,
                    org = escape(&org_name)
                )
            } else {
                String::new()
            };
            format!(
                r#"{org_form}<form method="post" action="space/name" data-async data-then="reload" data-done="Renamed" class="row" style="margin-top:8px"><label class="muted">This space <input name="name" value="{name}" required maxlength="60"></label><span class="fw"><button class="btn small ghost">Rename</button></span></form>"#,
                name = escape(&name)
            )
        } else {
            String::new()
        };
        let org_html = format!(
            r#"<section class="card"><h3>{org}</h3><p class="muted">The organisation this space belongs to; its members are in all of its spaces.</p><div class="row">{switch_list}</div><div style="margin-top:12px">{name_form}</div><div style="margin-top:8px">{new_space}</div></section>"#,
            org = escape(&org_name)
        );

        // Safety: asking for PPE before use (crate::safety).
        let ppe_on = store.ppe_checks(space).await?;
        let safety_html = format!(
            r#"<section class="card"><h3>Safety</h3><p class="muted">For a makerspace: before someone uses a machine that needs it, they tick off its PPE (laser safety glasses at a laser, safety glasses and hearing protection at a CNC). Each machine's list is in its Settings. At home you may leave it off.</p>{}</section>"#,
            if admin {
                format!(
                    r#"<form method="post" action="space/safety" data-async data-then="reload" data-done="Saved"><label><input type="checkbox" name="on" value="1"{checked} onchange="this.form.requestSubmit()"> Ask for PPE before use</label></form>"#,
                    checked = if ppe_on { " checked" } else { "" }
                )
            } else if ppe_on {
                "<p>On: machines ask for their PPE before use.</p>".to_owned()
            } else {
                "<p>Off.</p>".to_owned()
            }
        );

        // One's own account, and the server.
        let instance_html = if signed.as_ref().is_some_and(|s| s.instance_admin) {
            let open_spaces = store.open_spaces().await;
            format!(
                r#"<section class="card"><h3>This server</h3><p class="muted">You run it. Usually only you make organisations; on a hosted service anyone signed in may make their own.</p><form method="post" action="instance/open-spaces" data-async data-done="Saved"><label><input type="checkbox" name="open" value="1"{checked} onchange="this.form.requestSubmit()"> Anyone signed in may make an organisation</label></form>
<form method="post" action="spaces" data-async data-then="./" data-done="Organisation made" class="row" style="margin-top:12px"><input name="name" required maxlength="60" placeholder="Fablab Zuid" aria-label="Name of the new organisation"><span class="fw"><button class="btn small ghost">New organisation</button></span></form></section>"#,
                checked = if open_spaces { " checked" } else { "" }
            )
        } else {
            String::new()
        };
        let account_html = signed.as_ref().map_or(String::new(), |signed| format!(
            r#"<section class="card"><h3>Your account</h3><p class="muted">Signed in as <b>{display}</b> ({name}).</p><form method="post" action="account/password" data-async data-done="Password changed: signed out elsewhere" class="row"><input type="password" name="current" required autocomplete="current-password" placeholder="Current password" aria-label="Current password"><input type="password" name="new" required minlength="{min}" autocomplete="new-password" placeholder="New password" aria-label="New password"><input type="password" name="again" required minlength="{min}" autocomplete="new-password" placeholder="Again" aria-label="New password again"><span class="fw"><button class="btn small">Change password</button></span></form></section>"#,
            display = escape(&signed.display_name),
            name = escape(&signed.name),
            min = accounts::MIN_PASSWORD,
        ));

        // The tab asked for, or the first one this person has.
        let here = match (&tab, account) {
            (_, true) => "account",
            (Some(tab), _) => tab.as_str(),
            (None, _) if admin => "general",
            (None, _) => "permissions",
        };
        let mut tabs: Vec<(&str, &str)> = TABS.to_vec();
        if !cameras_html.is_empty() {
            tabs.insert(4, ("cameras", "Cameras"));
        }
        let permissions_html = format!(
            "{}{}{}",
            access_parts.matrix, access_parts.groups, access_parts.machines
        );
        let panel = match here {
            "account" if !account_html.is_empty() => Some(account_html.clone()),
            "permissions" => Some(permissions_html),
            _ if !admin => None,
            "general" => Some(format!("{org_html}{safety_html}{instance_html}")),
            "members" => Some(format!("{members_html}{invites_html}")),
            "access" => Some(crate::access_page::placeholder()),
            "cameras" if !cameras_html.is_empty() => Some(cameras_html.clone()),
            "tokens" if !tokens_html.is_empty() => Some(tokens_html.clone()),
            "audit" if !audit_html.is_empty() => Some(audit_html.clone()),
            _ => None,
        };
        let known = here == "account" || tabs.iter().any(|(id, _)| *id == here);
        let status = match (&panel, known) {
            (Some(_), _) => StatusCode::OK,
            (None, true) => StatusCode::FORBIDDEN,
            (None, false) => StatusCode::NOT_FOUND,
        };
        let title = match here {
            "account" => "Your account",
            _ if admin => "Space",
            _ => "My permissions",
        };
        let nav = if admin && here != "account" {
            let links: String = tabs
                .iter()
                .map(|(id, label)| {
                    format!(
                        r#"<a href="/space/{id}"{current}>{label}</a>"#,
                        current = if *id == here { r#" aria-current="page""# } else { "" }
                    )
                })
                .collect();
            format!(r#"<nav aria-label="Space"><div class="tabs">{links}</div></nav>"#)
        } else {
            String::new()
        };
        let body = format!(
            r#"<div class="head"><h1>{title}</h1><span class="tag">{name} · you: {role}</span></div>{nav}{panel}"#,
            name = escape(&name),
            role = access.role.as_str(),
            panel = panel.unwrap_or_else(|| match known {
                true => r#"<div class="banner warn" role="alert"><span>This page is for the space's staff.</span></div>"#.to_owned(),
                false => r#"<div class="banner warn" role="alert"><span>There is no such tab in Space.</span></div>"#.to_owned(),
            }),
        );

        Ok((status, format!(
            r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name} · Maker Connect</title>
<link rel="icon" href="/logo.svg" type="image/svg+xml">
<base href="/">
<style>
{STYLE}{SPACE_STYLE}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1">
  {body}
</main>
<script>
  // The matrix: a tick gives (operate) or takes a group's permission, at once; the level
  // under it switches between operate (everything) and use (jobs only).
  const tick = (user, group, body) => fetch(`permissions/groups/${{group}}/people/${{user}}`, {{ method: "POST", headers: {{ "x-requested-with": "fetch" }}, body: new URLSearchParams(body) }});
  document.addEventListener("change", async (e) => {{
    const box = e.target.closest("[data-mx-user]");
    if (!box) return;
    const level = box.closest("td").querySelector("[data-mx-level]");
    const r = await tick(box.dataset.mxUser, box.dataset.mxGroup, {{ on: box.checked ? "1" : "0" }});
    if (r.ok) {{
      level.textContent = box.checked ? (await r.text()) || "operate" : "";
      level.hidden = !box.checked;
      toast(box.checked ? "Permission given" : "Permission taken away");
    }} else {{
      box.checked = !box.checked;
      toast(`Not done: ${{await r.text()}}`, "bad");
    }}
  }});
  document.addEventListener("click", async (e) => {{
    const level = e.target.closest("[data-mx-level]");
    if (!level) return;
    const [user, group] = level.dataset.mxLevel.split(":");
    const next = level.textContent.trim() === "operate" ? "use" : "operate";
    const r = await tick(user, group, {{ on: "1", level: next }});
    if (r.ok) {{
      level.textContent = (await r.text()) || next;
      toast(next === "use" ? "Jobs only (use)" : "Everything (operate)");
    }} else toast(`Not done: ${{await r.text()}}`, "bad");
  }});
  // Someone's permissions: a window per person.
  document.addEventListener("click", (e) => {{
    const open = e.target.closest("[data-open-perm]");
    if (open) document.querySelector(`[data-perm="${{open.dataset.openPerm}}"]`).showModal();
    if (e.target.closest("[data-close]")) e.target.closest("dialog")?.close();
  }});
  fetch("api/machines").then((r) => r.json()).then(summarise).catch(() => {{}});
  // A link or a token made: shown once, here.
  document.addEventListener("saved", (event) => {{
    const kind = event.target.dataset.secret;
    const data = event.detail.data;
    if (!kind || !data) return;
    const box = document.querySelector(`[data-secret-box="${{kind}}"]`);
    const text = kind === "invite" ? new URL(data.path, location.href).href : data.token;
    box.querySelector("[data-secret-text]").textContent = text;
    const moonraker = box.querySelector("[data-secret-moonraker]");
    if (moonraker) moonraker.textContent = `[spoolman]\nserver: ${{location.protocol}}//${{location.host}}/t/${{data.token}}`;
    box.hidden = false;
    box.querySelector("[data-copy-secret]").onclick = async (e) => {{
      try {{ await navigator.clipboard.writeText(text); e.target.textContent = "Copied"; }}
      catch {{ e.target.textContent = "Select and copy it"; }}
    }};
  }});
</script>
</body>
</html>"##,
            header = header(&name, "", if account { "account" } else { "space" }),
            name = escape(&name),
        )))
    }
    .await;
    match built {
        Ok((status, html)) => (status, Html(html)).into_response(),
        Err(error) => failed(error),
    }
}

async fn audit(access: Access, State(store): State<Store>) -> Response {
    match store.audit_log(access.space, 500).await {
        Ok(lines) => Json(lines).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct NameForm {
    name: String,
}

fn clean_name(name: &str) -> Result<String, Box<Response>> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(Box::new(
            (StatusCode::BAD_REQUEST, "a name of 1 to 60 characters").into_response(),
        ));
    }
    Ok(name.to_owned())
}

/// The organisation's name: for an admin of the organisation (not of one space).
async fn rename_organisation(
    access: Access,
    State(store): State<Store>,
    Form(form): Form<NameForm>,
) -> Response {
    let name = match clean_name(&form.name) {
        Ok(name) => name,
        Err(r) => return *r,
    };
    let done: sqlx::Result<bool> = async {
        if store
            .org_role_of(access.user_id, access.space)
            .await?
            .is_none_or(|r| r < Role::Admin)
        {
            return Ok(false);
        }
        store.rename_organisation(access.space, &name).await?;
        Ok(true)
    }
    .await;
    match done {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => refused("only an admin of the organisation renames it"),
        Err(error) => failed(error),
    }
}

async fn rename(
    access: Access,
    State(store): State<Store>,
    Form(form): Form<NameForm>,
) -> Response {
    let name = match clean_name(&form.name) {
        Ok(name) => name,
        Err(r) => return *r,
    };
    match store.rename_space(access.space, &name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct InviteForm {
    role: String,
    scope: Option<String>,
}

async fn new_invite(
    access: Access,
    State(store): State<Store>,
    Form(form): Form<InviteForm>,
) -> Response {
    let Some(role) = Role::parse(&form.role) else {
        return (StatusCode::BAD_REQUEST, "which role?").into_response();
    };
    if role > access.role {
        return refused("not a role above your own");
    }
    let whole = form.scope.as_deref() != Some("space");
    if whole
        && store
            .org_role_of(access.user_id, access.space)
            .await
            .ok()
            .flatten()
            .is_none_or(|r| r < Role::Admin || r < role)
    {
        return refused("only an admin of the organisation invites into all of it");
    }
    match store
        .create_invite(access.space, role, access.user_id, whole)
        .await
    {
        Ok(token) => {
            Json(serde_json::json!({ "path": format!("/invite/{token}") })).into_response()
        }
        Err(error) => failed(error),
    }
}

async fn revoke_invite(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Response {
    match store.revoke_invite(access.space, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such link").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct RoleForm {
    role: String,
    /// `organisation` (all its spaces) or `space` (this one).
    scope: String,
}

/// A member's role in the organisation or in this space; `none` takes that one away. Nobody
/// gives a role above their own or changes someone above them; an organisation keeps an
/// owner.
async fn set_role(
    access: Access,
    State(store): State<Store>,
    Path(user): Path<i64>,
    Form(form): Form<RoleForm>,
) -> Response {
    let role = match form.role.as_str() {
        "none" => None,
        r => match Role::parse(r) {
            Some(role) => Some(role),
            None => return (StatusCode::BAD_REQUEST, "which role?").into_response(),
        },
    };
    if role.is_some_and(|r| r > access.role) {
        return refused("not a role above your own");
    }
    let done: sqlx::Result<Result<(), &'static str>> = async {
        let space = access.space;
        let theirs = store.role_of(user, space).await?;
        if theirs.is_some_and(|r| r > access.role) {
            return Ok(Err("not someone above you"));
        }
        if form.scope == "organisation" {
            if store
                .org_role_of(access.user_id, access.space)
                .await?
                .is_none_or(|r| r < Role::Admin)
            {
                return Ok(Err(
                    "only an admin of the organisation changes roles in all of it",
                ));
            }
            let was = store.org_role_of(user, access.space).await?;
            if was == Some(Role::Owner)
                && role != Some(Role::Owner)
                && store.owners(space).await? <= 1
            {
                return Ok(Err(
                    "the organisation needs an owner: make another one first",
                ));
            }
            match role {
                Some(role) => {
                    store
                        .set_org_membership(user, access.space, role.as_str())
                        .await?
                }
                None => {
                    store.remove_member(space, user, true).await?;
                }
            }
        } else {
            match role {
                Some(role) => store.set_membership(user, space, role.as_str()).await?,
                None => {
                    store.remove_member(space, user, false).await?;
                }
            }
        }
        Ok(Ok(()))
    }
    .await;
    match done {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(why)) => refused(why),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct RemoveForm {
    scope: Option<String>,
}

async fn remove_member(
    access: Access,
    State(store): State<Store>,
    Path(user): Path<i64>,
    Form(form): Form<RemoveForm>,
) -> Response {
    let whole = form.scope.as_deref() == Some("organisation");
    let done: sqlx::Result<Result<bool, &'static str>> = async {
        let space = access.space;
        if user == access.user_id {
            return Ok(Err("not yourself"));
        }
        if store
            .role_of(user, space)
            .await?
            .is_some_and(|r| r > access.role)
        {
            return Ok(Err("not someone above you"));
        }
        if whole {
            if store
                .org_role_of(access.user_id, access.space)
                .await?
                .is_none_or(|r| r < Role::Admin)
            {
                return Ok(Err(
                    "only an admin of the organisation takes someone out of all of it",
                ));
            }
            if store.org_role_of(user, access.space).await? == Some(Role::Owner)
                && store.owners(space).await? <= 1
            {
                return Ok(Err("the organisation needs an owner"));
            }
        }
        Ok(Ok(store.remove_member(space, user, whole).await?))
    }
    .await;
    match done {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => (StatusCode::NOT_FOUND, "not a member").into_response(),
        Ok(Err(why)) => refused(why),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct TokenForm {
    name: String,
    role: String,
}

async fn new_token(
    access: Access,
    State(store): State<Store>,
    Form(form): Form<TokenForm>,
) -> Response {
    if access.token_id.is_some() {
        return refused("a token does not make tokens");
    }
    let name = match clean_name(&form.name) {
        Ok(name) => name,
        Err(r) => return *r,
    };
    let Some(role) = Role::parse(&form.role).filter(|r| *r <= Role::Admin) else {
        return (StatusCode::BAD_REQUEST, "guest, member or admin").into_response();
    };
    if role > access.role {
        return refused("not a role above your own");
    }
    match store
        .create_token(access.space, access.user_id, &name, role)
        .await
    {
        Ok(token) => Json(serde_json::json!({ "token": token })).into_response(),
        Err(error) => failed(error),
    }
}

async fn revoke_token(access: Access, State(store): State<Store>, Path(id): Path<i64>) -> Response {
    match store.revoke_token(access.space, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such token").into_response(),
        Err(error) => failed(error),
    }
}

// ---- joining, choosing a space, one's own account ---------------------------------------

/// The session behind a request on a route the guard lets through unsigned (`/invite/…`).
async fn session_of(store: &Store, headers: &HeaderMap) -> Option<Signed> {
    let token = crate::auth::cookie(headers, crate::auth::COOKIE)?;
    store.session(&token).await.ok().flatten()
}

fn plain_page(title: &str, body: &str) -> Html<String> {
    Html(crate::auth::auth_page(title, body))
}

async fn invite_page(
    State(store): State<Store>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let invite = match store.invite(&token).await {
        Ok(invite) => invite,
        Err(error) => return failed(error),
    };
    let Some(invite) = invite.filter(|i| i.open()) else {
        return (
            StatusCode::NOT_FOUND,
            plain_page("Invite link", r#"<div class="banner warn" role="alert"><span>This link is not (or no longer) valid: used, revoked, or more than a week old. Ask for a new one.</span></div>"#),
        )
            .into_response();
    };
    let into = match invite.whole_organisation {
        true => store
            .organisation_name(invite.space)
            .await
            .unwrap_or_default(),
        false => invite.space_name.clone(),
    };
    let what = format!(
        "<p class=\"lede\"><b>{by}</b> invites you into <b>{into}</b> as <b>{role}</b>.</p>",
        by = escape(&invite.by),
        into = escape(&into),
        role = invite.role.as_str()
    );
    let body = match session_of(&store, &headers).await {
        Some(signed) => format!(
            r#"{what}<div class="card"><form method="post"><span class="fw" style="width:100%"><button class="btn primary">Join as {name}</button></span></form><p style="margin-top:var(--s-3)"><small>Not you? <a href="/login?next=/invite/{token}">Sign in as someone else</a>.</small></p></div>"#,
            name = escape(&signed.display_name),
            token = escape(&token),
        ),
        None => format!(
            r#"{what}<div class="card"><form method="post">
<label class="field">Your name<input name="display_name" maxlength="60" autocomplete="name" required autofocus></label>
<label class="field">Sign-in name<input name="name" maxlength="40" autocomplete="username" autocapitalize="none" required aria-describedby="name-why"><small id="name-why">Letters, digits, dots, dashes or underscores.</small></label>
<label class="field">Password<input name="password" type="password" minlength="{min}" autocomplete="new-password" required></label>
<label class="field">Password again<input name="again" type="password" minlength="{min}" autocomplete="new-password" required></label>
<span class="fw" style="width:100%"><button class="btn primary">Make my account and join</button></span></form>
<p style="margin-top:var(--s-3)"><small>Have an account here? <a href="/login?next=/invite/{token}">Sign in</a> first.</small></p></div>"#,
            min = accounts::MIN_PASSWORD,
            token = escape(&token),
        ),
    };
    plain_page("Join", &body).into_response()
}

#[derive(Deserialize)]
struct JoinForm {
    display_name: Option<String>,
    name: Option<String>,
    password: Option<String>,
    again: Option<String>,
}

async fn join(
    State(store): State<Store>,
    Path(token): Path<String>,
    Extension(Client(client)): Extension<Client>,
    headers: HeaderMap,
    Form(form): Form<JoinForm>,
) -> Response {
    if !from_this_site(&Method::POST, &headers) {
        return refused("refused: this came from another site");
    }
    let (invite, signed) = (
        store
            .invite(&token)
            .await
            .ok()
            .flatten()
            .filter(|i| i.open()),
        session_of(&store, &headers).await,
    );
    let Some(invite) = invite else {
        return (
            StatusCode::NOT_FOUND,
            "this link is not (or no longer) valid",
        )
            .into_response();
    };
    // Signed in already: in, and the session goes to the space.
    if let Some(signed) = signed {
        return match store.use_invite(&invite, signed.user_id).await {
            Ok(true) => {
                let _ = store
                    .set_session_space(signed.user_id, signed.session_id, invite.space)
                    .await;
                Redirect::to("/").into_response()
            }
            Ok(false) => (StatusCode::CONFLICT, "the link was used meanwhile").into_response(),
            Err(error) => failed(error),
        };
    }
    let display_name = form.display_name.unwrap_or_default().trim().to_owned();
    let name = form.name.unwrap_or_default().trim().to_owned();
    let password = form.password.unwrap_or_default();
    let problem = if display_name.is_empty() || display_name.chars().count() > 60 {
        Some("Your name needs 1 to 60 characters.".to_owned())
    } else if let Some(p) = accounts::name_problem(&name) {
        Some(format!("Not that sign-in name: {p}."))
    } else if let Some(p) = accounts::password_problem(&password) {
        Some(format!("Not that password: {p}."))
    } else if Some(&password) != form.again.as_ref() {
        Some("The two passwords differ.".to_owned())
    } else if store.user_id(&name).await.ok().flatten().is_some() {
        Some(
            "That sign-in name is taken: choose another, or sign in first if it is yours."
                .to_owned(),
        )
    } else {
        None
    };
    if let Some(problem) = problem {
        return (
            StatusCode::BAD_REQUEST,
            plain_page(
                "Join",
                &format!(
                    r#"<div class="banner bad" role="alert"><span>{}</span></div><p><a href="/invite/{}">Back</a></p>"#,
                    escape(&problem),
                    escape(&token)
                ),
            ),
        )
            .into_response();
    }
    let hash = tokio::task::spawn_blocking(move || accounts::hash_password(&password))
        .await
        .unwrap_or_default();
    let user_agent = headers
        .get(h::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let made = store
        .join_by_invite(&invite, &name, &display_name, &hash, user_agent, &client)
        .await;
    match made {
        Ok(Some(token)) => {
            println!("{name} joined by invite ({client})");
            crate::auth::signed_in(&token, &headers, "/")
        }
        Ok(None) => (StatusCode::CONFLICT, "the link was used meanwhile").into_response(),
        Err(error) => failed(error),
    }
}

/// Signed in, in no space: how to get into one.
async fn welcome(State(store): State<Store>, Extension(signed): Extension<Signed>) -> Response {
    if store
        .spaces_of(signed.user_id)
        .await
        .is_ok_and(|s| !s.is_empty())
    {
        return Redirect::to("/").into_response();
    }
    let make = if signed.instance_admin || store.open_spaces().await {
        r#"<div class="card"><h2>Or start your own</h2><form method="post" action="/spaces"><label class="field">Name of your organisation<input name="name" maxlength="60" required placeholder="Fablab Zuid"></label><span class="fw"><button class="btn primary">Make it</button></span></form></div>"#
    } else {
        ""
    };
    plain_page(
        "Welcome",
        &format!(
            r#"<p class="lede">Hello {name}. You are not in a space yet: ask someone in one for an invite link.</p>{make}<form method="post" action="/logout" style="margin-top:var(--s-4)"><span class="fw"><button class="btn">Sign out</button></span></form>"#,
            name = escape(&signed.display_name)
        ),
    )
    .into_response()
}

#[derive(Deserialize)]
struct SwitchForm {
    space: i64,
}

async fn switch(
    State(store): State<Store>,
    Extension(signed): Extension<Signed>,
    Form(form): Form<SwitchForm>,
) -> Response {
    match store.role_of(signed.user_id, form.space).await {
        Ok(Some(_)) => match store
            .set_session_space(signed.user_id, signed.session_id, form.space)
            .await
        {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => failed(error),
        },
        Ok(None) => (StatusCode::NOT_FOUND, "not a space of yours").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct NewSpaceForm {
    name: String,
    /// `this`: a new space in the organisation of the space one is in; else a new
    /// organisation (with a first space of the same name).
    organisation: Option<String>,
}

async fn new_space(
    State(store): State<Store>,
    Extension(signed): Extension<Signed>,
    access: Option<Extension<Access>>,
    headers: HeaderMap,
    Form(form): Form<NewSpaceForm>,
) -> Response {
    let name = match clean_name(&form.name) {
        Ok(name) => name,
        Err(r) => return *r,
    };
    let made: sqlx::Result<Result<i64, &'static str>> = async {
        if form.organisation.as_deref() == Some("this") {
            let Some(Extension(access)) = &access else {
                return Ok(Err("not in a space"));
            };
            if store
                .org_role_of(signed.user_id, access.space)
                .await?
                .is_none_or(|r| r < Role::Admin)
            {
                return Ok(Err("only an admin of the organisation makes spaces in it"));
            }
            let space = store.create_space(access.space, &name).await?;
            let _ = store.audit(access, "made a space", &name, "").await;
            Ok(Ok(space))
        } else {
            if !(signed.instance_admin || store.open_spaces().await) {
                return Ok(Err("only who runs the server makes organisations here"));
            }
            Ok(Ok(store
                .create_organisation(&name, &name, signed.user_id)
                .await?
                .1))
        }
    }
    .await;
    match made {
        Ok(Ok(space)) => {
            let _ = store
                .set_session_space(signed.user_id, signed.session_id, space)
                .await;
            if headers.contains_key("x-requested-with") {
                StatusCode::NO_CONTENT.into_response()
            } else {
                Redirect::to("/").into_response()
            }
        }
        Ok(Err(why)) => refused(why),
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    new: String,
    again: String,
}

/// A new password: the current one first; every other session ends, this one goes on.
async fn password(
    State(store): State<Store>,
    Extension(signed): Extension<Signed>,
    headers: HeaderMap,
    Extension(Client(client)): Extension<Client>,
    Form(form): Form<PasswordForm>,
) -> Response {
    if let Some(problem) = accounts::password_problem(&form.new) {
        return (StatusCode::BAD_REQUEST, problem).into_response();
    }
    if form.new != form.again {
        return (StatusCode::BAD_REQUEST, "the two new passwords differ").into_response();
    }
    let hash = match store.credentials(&signed.name).await {
        Ok(Some(c)) => c.hash,
        Ok(None) => String::new(),
        Err(error) => return failed(error),
    };
    let (current, new) = (form.current, form.new);
    let checked = tokio::task::spawn_blocking(move || {
        accounts::verify_password(&current, &hash).then(|| accounts::hash_password(&new))
    })
    .await
    .ok()
    .flatten();
    let Some(new_hash) = checked else {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        return (StatusCode::FORBIDDEN, "that is not your current password").into_response();
    };
    let user_agent = headers
        .get(h::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    // A new password ends every session; this one is made again.
    let done: sqlx::Result<String> = async {
        let space = store.spaces_of(signed.user_id).await?.first().map(|s| s.id);
        store
            .change_password(signed.user_id, &new_hash, space, &user_agent, &client)
            .await
    }
    .await;
    match done {
        Ok(token) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(
                h::SET_COOKIE,
                crate::auth::session_cookie(Some(&token), &headers),
            );
            response
        }
        Err(error) => failed(error),
    }
}

#[derive(Deserialize)]
struct OpenForm {
    open: Option<String>,
}

async fn open_spaces(State(store): State<Store>, Form(form): Form<OpenForm>) -> Response {
    match store.set_open_spaces(form.open.is_some()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use crate::permissions_page::end_of_day;

    #[test]
    fn days_end_where_they_should() {
        assert_eq!(end_of_day("1970-01-01"), Some(86_399));
        // 2026-11-06 00:00 UTC is 1793923200.
        assert_eq!(end_of_day("2026-11-06"), Some(1_793_923_200 + 86_399));
        assert_eq!(end_of_day("2024-02-29"), Some(1_709_164_800 + 86_399));
        assert_eq!(end_of_day("2026-13-01"), None);
        assert_eq!(end_of_day("tomorrow"), None);
    }
}
