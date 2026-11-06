//! A space's own page (its members, invite links, who may use which machine, API tokens,
//! the audit log), joining by an invite link, choosing or making a space, and one's own
//! password. What each needs is in [`crate::access::RULES`]; the rules within (no role
//! above one's own, an organisation keeps an owner) are here.

use crate::access::{Access, Role};
use crate::accounts::{self, Signed};
use crate::auth::{Client, from_this_site};
use crate::{STYLE, Shared, escape, failed, header};
use axum::{
    Extension, Form, Json, Router,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, header as h},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/space", get(page))
        .route("/api/space/audit", get(audit))
        .route("/space/name", post(rename))
        .route("/organisation/name", post(rename_organisation))
        .route("/space/invites", post(new_invite))
        .route("/space/invites/{id}/revoke", post(revoke_invite))
        .route("/space/members/{user}/role", post(set_role))
        .route("/space/members/{user}/remove", post(remove_member))
        .route("/space/tokens", post(new_token))
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

pub(crate) const SPACE_STYLE: &str = r#"
  .tabs { display: flex; gap: 4px; margin: 14px 0 4px; border-bottom: 1px solid var(--line); overflow-x: auto; overflow-y: hidden; scrollbar-width: none; }
  .tabs::-webkit-scrollbar { display: none; }
  .tabs a { padding: 10px 14px; text-decoration: none; color: var(--muted); font-weight: 600; border-bottom: 2px solid transparent; margin-bottom: -1px; white-space: nowrap; }
  .tabs a[aria-selected=true] { color: var(--ink); border-color: var(--accent); }
  .space-head { display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px 14px; }
  .space-head h2 { margin: 0; }
  .section { background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 18px 20px; margin: 18px 0; }
  .section h3 { margin: 0 0 4px; font-size: 17px; }
  .section > p.sub { margin: 0 0 14px; color: var(--muted); font-size: 14px; }
  table.list { width: 100%; border-collapse: collapse; font-size: 14px; }
  table.list th { text-align: left; color: var(--muted); font-weight: 600; font-size: 12px; text-transform: uppercase; letter-spacing: .5px; padding: 6px 8px; border-bottom: 1px solid var(--line); }
  table.list td { padding: 8px; border-bottom: 1px solid var(--line); vertical-align: middle; }
  table.list tr:last-child td { border-bottom: 0; }
  table.list form { margin: 0; display: inline-flex; gap: 6px; flex-wrap: nowrap; }
  table.list small, .muted { color: var(--muted); }
  .chip { display: inline-block; padding: 2px 8px; border-radius: 999px; background: var(--line); font-size: 12px; font-weight: 600; }
  .chip.owner, .chip.admin { background: color-mix(in srgb, var(--accent) 20%, transparent); }
  .ghost { background: transparent; color: var(--ink); border: 1px solid var(--line); }
  .danger { background: transparent; color: var(--bad); border: 1px solid color-mix(in srgb, var(--bad) 40%, transparent); }
  button.small, select.small { padding: 4px 8px; font-size: 13px; }
  .secret { margin: 10px 0 0; padding: 12px; border: 1px solid var(--accent); border-radius: 10px; background: color-mix(in srgb, var(--accent) 8%, transparent); }
  .secret pre { margin: 6px 0; white-space: pre-wrap; overflow-wrap: anywhere; user-select: all; }
  .pchips { margin: 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 6px; }
  .pchips li { display: inline-flex; align-items: center; gap: 4px; padding: 2px 4px 2px 10px; border: 1px solid var(--line); border-radius: 999px; font-size: 13px; white-space: nowrap; }
  .pchips li.group { border-color: color-mix(in srgb, var(--accent) 45%, var(--line)); }
  .pchips form { margin: 0; }
  .pchips button { padding: 0 6px; background: none; color: var(--muted); font-size: 15px; border: 0; }
  .pchips li.add { border-style: dashed; padding: 0; }
  .pchips li.add select { border: 0; background: transparent; font-size: 13px; padding: 3px 8px; color: var(--muted); }
  td.may { min-width: 220px; }
  table.list td.since { white-space: nowrap; }
  .give { margin-top: 6px; }
  .give summary { cursor: pointer; color: var(--accent); font-size: 13px; font-weight: 600; list-style: none; display: inline-block; }
  .give summary::-webkit-details-marker { display: none; }
  .give form { display: flex !important; flex-wrap: wrap !important; gap: 6px; align-items: center; margin-top: 6px !important; }
  .give label { font-size: 13px; }
  /* A chip that opens to change it, and "+ …" that opens a small form under it. */
  .pchips li { position: relative; }
  .pchips details.edit summary { list-style: none; cursor: pointer; }
  .pchips details.edit summary::-webkit-details-marker { display: none; }
  .pchips li.add details.edit summary { color: var(--accent); font-weight: 600; padding: 2px 10px 2px 0; }
  .pchips details.edit[open] > form { position: absolute; top: calc(100% + 6px); left: 0; z-index: 30; display: flex; flex-wrap: wrap; gap: 6px; align-items: center; width: max-content; max-width: min(520px, 80vw); padding: 10px; background: var(--card); border: 1px solid var(--line); border-radius: 10px; box-shadow: 0 10px 30px rgb(0 0 0 / .25); }
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
  .chip.planned { background: color-mix(in srgb, var(--warn) 18%, transparent); color: var(--warn); }
  .a-head { display: flex; align-items: center; gap: 10px; }
  .a-head h3 { margin: 0 0 4px; }
  .a-flow { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: 10px; counter-reset: a; }
  .a-flow li { display: grid; gap: 4px; padding: 12px 14px; border: 1px solid var(--line); border-radius: 12px; }
  .a-flow small { color: var(--muted); font-size: 13px; }
  .a-n { display: inline-grid; place-items: center; width: 22px; height: 22px; border-radius: 50%; background: var(--line); font-size: 12px; font-weight: 700; }
  .a-form { display: flex; flex-wrap: wrap; gap: 8px; }
  .a-form input { flex: 1 1 220px; }
  .access-plan button:disabled { opacity: .45; cursor: not-allowed; }
  .a-example { opacity: .7; }
  .a-note { margin: 8px 0 0; font-size: 12px; }
  .a-state { font-size: 12px; font-weight: 600; } .a-state.ok { color: var(--accent); } .a-state.warn { color: var(--warn); }
  .a-log { list-style: none; margin: 0; padding: 0; font-size: 14px; }
  .a-log li { padding: 6px 0; border-top: 1px solid var(--line); } .a-log li:first-child { border-top: 0; }
  .a-log time { color: var(--muted); margin-right: 10px; font-variant-numeric: tabular-nums; }
  .mx-wrap { overflow-x: auto; }
  .mx-table th.mx, .mx-table td.mx { text-align: center; white-space: nowrap; }
  .mx-table td.mx label { display: inline-flex; flex-direction: column; align-items: center; gap: 2px; cursor: pointer; min-width: 48px; }
  .mx-table td.mx input { width: 18px; height: 18px; margin: 0; }
  .mx-table td.mx small { font-size: 11px; color: var(--muted); min-height: 1em; }
  .mx-table td.mx-more { text-align: right; white-space: nowrap; }
  .gcards { display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 12px; }
  .gcard { border: 1px solid var(--line); border-radius: 12px; padding: 12px 14px; display: grid; gap: 10px; align-content: start; }
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
  .machine-access .grants li { display: inline-flex; align-items: center; gap: 6px; padding: 3px 4px 3px 10px; border: 1px solid var(--line); border-radius: 999px; font-size: 13px; }
  .machine-access .grants form { margin: 0; }
  .machine-access .g-label { margin: 10px 0 0; font-size: 12px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; color: var(--muted); }
  .machine-access header small { margin-right: auto; }
  .machine-access .grants button { padding: 0 6px; background: none; color: var(--muted); font-size: 15px; }
  .audit li { padding: 6px 0; border-bottom: 1px solid var(--line); font-size: 14px; list-style: none; }
  .audit { padding: 0; margin: 0; max-height: 420px; overflow-y: auto; }
  .audit time { color: var(--muted); font-variant-numeric: tabular-nums; margin-right: 8px; }
  .switch { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
"#;

async fn page(
    access: Access,
    State(store): State<Shared>,
    signed: Option<Extension<Signed>>,
) -> Response {
    // With an API token (no session) the page shows all but one's own account.
    let signed = signed.map(|Extension(s)| s);
    let store = store.lock().unwrap();
    let built = (|| -> rusqlite::Result<String> {
        let space = access.space;
        let name = store.space_name(space)?;
        let organisation = store.organisation_of(space)?;
        let org_name = store.organisation_name(organisation)?;
        let org_role = store.org_role_of(access.user_id, organisation)?;
        let admin = access.role >= Role::Admin;
        let org_admin = org_role.is_some_and(|r| r >= Role::Admin);
        let siblings: Vec<_> = store
            .spaces_of(access.user_id)?
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
        let members = store.members(space)?;
        let access_parts = crate::permissions_page::parts(&store, &access)?;
        let mut rows = String::new();
        for m in &members {
            let you = m.user_id == access.user_id;
            let editable = admin && m.role <= access.role;
            let layer = |label: &str, scope: &str, have: Option<Role>, allowed: bool| {
                if editable && allowed {
                    format!(
                        r#"<form method="post" action="space/members/{id}/role" data-async data-then="reload" data-done="Role changed"><input type="hidden" name="scope" value="{scope}"><select class="small" name="role" aria-label="Role {label}" onchange="this.form.requestSubmit()">{opts}</select></form>"#,
                        id = m.user_id,
                        opts = roles(access.role, have, true),
                    )
                } else {
                    have.map_or("<small>—</small>".to_owned(), |r| {
                        format!(r#"<span class="chip {r}">{r}</span>"#, r = r.as_str())
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
                since = day(m.since),
                remove = if editable && !you {
                    format!(
                        r#"<form method="post" action="space/members/{id}/remove" data-async data-then="reload" data-done="Removed" data-confirm="Take {who} out of {what}?"><input type="hidden" name="scope" value="{scope}"><button class="small danger">Remove</button></form>"#,
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
        let matrix = &access_parts.matrix;
        let members_html = format!(
            r#"<section class="section"><h3>Members</h3><p class="sub">A role in the organisation counts in all of its spaces; a role in this space only here. The higher one counts. Owners and admins manage the space and every machine; using a machine takes permission, for everyone (below), unless it is open; guests look only.</p>
<table class="list"><thead><tr><th>Who</th><th>Organisation</th><th>This space</th><th>Since</th><th></th></tr></thead><tbody>{rows}</tbody></table></section>{matrix}"#
        );

        // Invite links.
        let invites_html = if admin {
            let mut list = String::new();
            for i in store.invites(space)?.iter().filter(|i| i.open()).take(20) {
                list += &format!(
                    r#"<tr><td><span class="chip {r}">{r}</span></td><td>{scope}</td><td><small>{by}, until {until}</small></td><td><form method="post" action="space/invites/{id}/revoke" data-async data-then="reload" data-done="Revoked"><button class="small ghost">Revoke</button></form></td></tr>"#,
                    r = i.role.as_str(),
                    scope = if i.whole_organisation {
                        escape(&org_name)
                    } else {
                        escape(&name)
                    },
                    by = escape(&i.by),
                    until = day(i.expires_at),
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
                r#"<section class="section"><h3>Invite links</h3><p class="sub">A link for one person, valid a week: they make their account (or sign in) and are in. No email needed: send it however you like.</p>
<form method="post" action="space/invites" data-async data-done="Link made" data-secret="invite" class="switch"><label>As <select name="role">{roles}</select></label><label>Into {scope_choice}</label><button>Make a link</button></form>
<div data-secret-box="invite" hidden class="secret"><b>The link, shown once:</b><pre data-secret-text></pre><button type="button" class="small ghost" data-copy-secret>Copy</button></div>
{table}</section>"#,
                roles = roles(access.role, Some(Role::Member), false),
                table = if list.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<table class="list" style="margin-top:12px"><thead><tr><th>Role</th><th>Into</th><th>Made</th><th></th></tr></thead><tbody>{list}</tbody></table>"#
                    )
                },
            )
        } else {
            String::new()
        };

        // API tokens.
        let tokens_html = if admin {
            let mut list = String::new();
            for t in store.tokens(space)? {
                list += &format!(
                    r#"<tr><td><b>{name}</b></td><td><span class="chip {r}">{r}</span></td><td><small>{by}, {made}</small></td><td><small>{used}</small></td><td><form method="post" action="space/tokens/{id}/revoke" data-async data-then="reload" data-done="Revoked" data-confirm="Revoke {name}? What uses it stops working."><button class="small danger">Revoke</button></form></td></tr>"#,
                    name = escape(&t.name),
                    r = t.role.as_str(),
                    by = escape(&t.by),
                    made = day(t.created_at),
                    used = t
                        .last_used
                        .map_or("never used".to_owned(), |u| format!("used {}", day(u))),
                    id = t.id,
                );
            }
            format!(
                r#"<section class="section"><h3>API tokens</h3><p class="sub">For programs: Moonraker's and OctoPrint's Spoolman (they take the address with the token in it), LaserFoundry, scripts (<code>Authorization: Bearer</code>). A token acts in this space with its role, never more than yours.</p>
<form method="post" action="space/tokens" data-async data-done="Token made" data-secret="token" class="switch"><input name="name" required maxlength="60" placeholder="Voron's Moonraker" aria-label="Name"><select name="role" aria-label="Role">{roles}</select><button>Make a token</button></form>
<div data-secret-box="token" hidden class="secret"><b>The token, shown once:</b><pre data-secret-text></pre><span class="muted">For Moonraker, in <code>moonraker.conf</code>:</span><pre data-secret-moonraker></pre><button type="button" class="small ghost" data-copy-secret>Copy the token</button></div>
{table}</section>"#,
                roles = roles(access.role.min(Role::Admin), Some(Role::Member), false),
                table = if list.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<table class="list" style="margin-top:12px"><thead><tr><th>Name</th><th>Role</th><th>Made</th><th>Used</th><th></th></tr></thead><tbody>{list}</tbody></table>"#
                    )
                },
            )
        } else {
            String::new()
        };

        // The audit log.
        let audit_html = if admin {
            let mut lines = String::new();
            for line in store.audit_log(space, 150)? {
                lines += &format!(
                    r#"<li><time>{at}</time><b>{actor}</b> {action} {target}{detail}</li>"#,
                    at = escape(
                        &crate::spoolman::iso(line.at)
                            .replace('T', " ")
                            .chars()
                            .take(16)
                            .collect::<String>()
                    ),
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
                r#"<section class="section"><h3>Audit log</h3><p class="sub">Who changed what in this space, newest first (times in UTC).</p><ul class="audit">{}</ul></section>"#,
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
                    format!(r#"<span class="chip">{}</span>"#, escape(&s.name))
                } else {
                    format!(
                        r#"<form method="post" action="spaces/switch" data-async data-then="./"><input type="hidden" name="space" value="{id}"><button class="small ghost">{name}</button></form>"#,
                        id = s.id,
                        name = escape(&s.name)
                    )
                }
            })
            .collect();
        let new_space = if org_admin {
            format!(
                r#"<form method="post" action="spaces" data-async data-then="./" data-done="Space made" class="switch"><input type="hidden" name="organisation" value="this"><input name="name" required maxlength="60" placeholder="A second location" aria-label="Name of the new space"><button class="small">New space in {org}</button></form>"#,
                org = escape(&org_name)
            )
        } else {
            String::new()
        };
        // Two names: the organisation's (who it is), and this space's (which place).
        let name_form = if admin {
            let org_form = if org_admin {
                format!(
                    r#"<form method="post" action="organisation/name" data-async data-then="reload" data-done="Renamed" class="switch"><label class="muted">Organisation <input name="name" value="{org}" required maxlength="60"></label><button class="small ghost">Rename</button></form>"#,
                    org = escape(&org_name)
                )
            } else {
                String::new()
            };
            format!(
                r#"{org_form}<form method="post" action="space/name" data-async data-then="reload" data-done="Renamed" class="switch" style="margin-top:8px"><label class="muted">This space <input name="name" value="{name}" required maxlength="60"></label><button class="small ghost">Rename</button></form>"#,
                name = escape(&name)
            )
        } else {
            String::new()
        };
        let org_html = format!(
            r#"<section class="section"><h3>{org}</h3><p class="sub">The organisation this space belongs to; its members are in all of its spaces.</p><div class="switch">{switch_list}</div><div style="margin-top:12px">{name_form}</div><div style="margin-top:8px">{new_space}</div></section>"#,
            org = escape(&org_name)
        );

        // Safety: asking for PPE before use (crate::safety).
        let ppe_on = store.ppe_checks(space);
        let safety_html = format!(
            r#"<section class="section"><h3>Safety</h3><p class="sub">For a makerspace: before someone uses a machine that needs it, they tick off its PPE (laser safety glasses at a laser, safety glasses and hearing protection at a CNC). Each machine's list is in its Settings. At home you may leave it off.</p>{}</section>"#,
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
            format!(
                r#"<section class="section"><h3>This server</h3><p class="sub">You run it. Usually only you make organisations; on a hosted service anyone signed in may make their own.</p><form method="post" action="instance/open-spaces" data-async data-done="Saved"><label><input type="checkbox" name="open" value="1"{checked} onchange="this.form.requestSubmit()"> Anyone signed in may make an organisation</label></form>
<form method="post" action="spaces" data-async data-then="./" data-done="Organisation made" class="switch" style="margin-top:12px"><input name="name" required maxlength="60" placeholder="Fablab Zuid" aria-label="Name of the new organisation"><button class="small ghost">New organisation</button></form></section>"#,
                checked = if store.open_spaces() { " checked" } else { "" }
            )
        } else {
            String::new()
        };
        let account_html = signed.as_ref().map_or(String::new(), |signed| format!(
            r#"<section class="section"><h3>Your account</h3><p class="sub">Signed in as <b>{display}</b> ({name}).</p><form method="post" action="account/password" data-async data-done="Password changed: signed out elsewhere" class="switch"><input type="password" name="current" required autocomplete="current-password" placeholder="Current password" aria-label="Current password"><input type="password" name="new" required minlength="{min}" autocomplete="new-password" placeholder="New password" aria-label="New password"><input type="password" name="again" required minlength="{min}" autocomplete="new-password" placeholder="Again" aria-label="New password again"><button class="small">Change password</button></form></section>"#,
            display = escape(&signed.display_name),
            name = escape(&signed.name),
            min = accounts::MIN_PASSWORD,
        ));

        Ok(format!(
            r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name} · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{SPACE_STYLE}
</style>
</head>
<body>
{header}
<main>
  <div class="space-head"><h2>{name}</h2><span class="chip {role}">you: {role}</span></div>
  <nav class="tabs" role="tablist"><a href="#members" role="tab">Members</a><a href="#permissions" role="tab">Permissions</a><a href="#access" role="tab">Access</a>{tokens_tab}{audit_tab}<a href="#general" role="tab">General</a></nav>
  <div role="tabpanel" data-panel="members">{members_html}{invites_html}</div>
  <div role="tabpanel" data-panel="access" hidden>{access_html}</div>
  <div role="tabpanel" data-panel="permissions" hidden>{groups_html}{machines_html}</div>
  <div role="tabpanel" data-panel="tokens" hidden>{tokens_html}</div>
  <div role="tabpanel" data-panel="audit" hidden>{audit_html}</div>
  <div role="tabpanel" data-panel="general" hidden>{org_html}{safety_html}{account_html}{instance_html}</div>
</main>
<script>
  // Tabs: the address says which (#members), so a reload stays there.
  function showTab() {{
    const tabs = [...document.querySelectorAll(".tabs a")];
    // Groups and machines are part of Permissions (older links said #groups, #machines).
    const hash = ["#groups", "#machines"].includes(location.hash) ? "#permissions" : location.hash;
    const tab = tabs.find((a) => a.hash === hash) || tabs[0];
    for (const a of tabs) a.setAttribute("aria-selected", String(a === tab));
    for (const p of document.querySelectorAll("[data-panel]")) p.hidden = `#${{p.dataset.panel}}` !== tab.hash;
  }}
  addEventListener("hashchange", showTab);
  showTab();
  // The matrix: a tick gives (use) or takes a group's permission, at once.
  document.addEventListener("change", async (e) => {{
    const box = e.target.closest("[data-mx-user]");
    if (!box) return;
    const r = await fetch(`permissions/groups/${{box.dataset.mxGroup}}/people/${{box.dataset.mxUser}}`, {{ method: "POST", headers: {{ "x-requested-with": "fetch" }}, body: new URLSearchParams({{ on: box.checked ? "1" : "0" }}) }});
    if (r.ok) {{
      box.nextElementSibling.textContent = box.checked ? (await r.text()) || "use" : "";
      toast(box.checked ? "Permission given" : "Permission taken away");
    }} else {{
      box.checked = !box.checked;
      toast(`Not done: ${{await r.text()}}`, "bad");
    }}
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
            header = header(&name, "", "space"),
            role = access.role.as_str(),
            groups_html = access_parts.groups,
            access_html = crate::access_page::placeholder(),
            machines_html = access_parts.machines,
            tokens_tab = if tokens_html.is_empty() {
                ""
            } else {
                r##"<a href="#tokens" role="tab">API tokens</a>"##
            },
            audit_tab = if audit_html.is_empty() {
                ""
            } else {
                r##"<a href="#audit" role="tab">Audit log</a>"##
            },
            name = escape(&name),
        ))
    })();
    match built {
        Ok(html) => Html(html).into_response(),
        Err(error) => failed(error),
    }
}

async fn audit(access: Access, State(store): State<Shared>) -> Response {
    match store.lock().unwrap().audit_log(access.space, 500) {
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
    State(store): State<Shared>,
    Form(form): Form<NameForm>,
) -> Response {
    let name = match clean_name(&form.name) {
        Ok(name) => name,
        Err(r) => return *r,
    };
    let store = store.lock().unwrap();
    let done = (|| -> rusqlite::Result<bool> {
        let organisation = store.organisation_of(access.space)?;
        if store
            .org_role_of(access.user_id, organisation)?
            .is_none_or(|r| r < Role::Admin)
        {
            return Ok(false);
        }
        store.rename_organisation(organisation, &name)?;
        Ok(true)
    })();
    match done {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => refused("only an admin of the organisation renames it"),
        Err(error) => failed(error),
    }
}

async fn rename(
    access: Access,
    State(store): State<Shared>,
    Form(form): Form<NameForm>,
) -> Response {
    let name = match clean_name(&form.name) {
        Ok(name) => name,
        Err(r) => return *r,
    };
    match store.lock().unwrap().rename_space(access.space, &name) {
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
    State(store): State<Shared>,
    Form(form): Form<InviteForm>,
) -> Response {
    let Some(role) = Role::parse(&form.role) else {
        return (StatusCode::BAD_REQUEST, "which role?").into_response();
    };
    if role > access.role {
        return refused("not a role above your own");
    }
    let store = store.lock().unwrap();
    let whole = form.scope.as_deref() != Some("space");
    if whole {
        let organisation = match store.organisation_of(access.space) {
            Ok(o) => o,
            Err(e) => return failed(e),
        };
        if store
            .org_role_of(access.user_id, organisation)
            .ok()
            .flatten()
            .is_none_or(|r| r < Role::Admin || r < role)
        {
            return refused("only an admin of the organisation invites into all of it");
        }
    }
    match store.create_invite(access.space, role, access.user_id, whole) {
        Ok(token) => {
            Json(serde_json::json!({ "path": format!("/invite/{token}") })).into_response()
        }
        Err(error) => failed(error),
    }
}

async fn revoke_invite(
    access: Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    match store.lock().unwrap().revoke_invite(access.space, id) {
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
    State(store): State<Shared>,
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
    let store = store.lock().unwrap();
    let done = (|| -> rusqlite::Result<Result<(), &'static str>> {
        let space = access.space;
        let organisation = store.organisation_of(space)?;
        let theirs = store.role_of(user, space)?;
        if theirs.is_some_and(|r| r > access.role) {
            return Ok(Err("not someone above you"));
        }
        if form.scope == "organisation" {
            if store
                .org_role_of(access.user_id, organisation)?
                .is_none_or(|r| r < Role::Admin)
            {
                return Ok(Err(
                    "only an admin of the organisation changes roles in all of it",
                ));
            }
            let was = store.org_role_of(user, organisation)?;
            if was == Some(Role::Owner) && role != Some(Role::Owner) && store.owners(space)? <= 1 {
                return Ok(Err(
                    "the organisation needs an owner: make another one first",
                ));
            }
            match role {
                Some(role) => store.set_org_membership(user, organisation, role.as_str())?,
                None => {
                    store.remove_member(space, user, true)?;
                }
            }
        } else {
            match role {
                Some(role) => store.set_membership(user, space, role.as_str())?,
                None => {
                    store.remove_member(space, user, false)?;
                }
            }
        }
        Ok(Ok(()))
    })();
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
    State(store): State<Shared>,
    Path(user): Path<i64>,
    Form(form): Form<RemoveForm>,
) -> Response {
    let whole = form.scope.as_deref() == Some("organisation");
    let store = store.lock().unwrap();
    let done = (|| -> rusqlite::Result<Result<bool, &'static str>> {
        let space = access.space;
        let organisation = store.organisation_of(space)?;
        if user == access.user_id {
            return Ok(Err("not yourself"));
        }
        if store.role_of(user, space)?.is_some_and(|r| r > access.role) {
            return Ok(Err("not someone above you"));
        }
        if whole {
            if store
                .org_role_of(access.user_id, organisation)?
                .is_none_or(|r| r < Role::Admin)
            {
                return Ok(Err(
                    "only an admin of the organisation takes someone out of all of it",
                ));
            }
            if store.org_role_of(user, organisation)? == Some(Role::Owner)
                && store.owners(space)? <= 1
            {
                return Ok(Err("the organisation needs an owner"));
            }
        }
        Ok(Ok(store.remove_member(space, user, whole)?))
    })();
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
    State(store): State<Shared>,
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
        .lock()
        .unwrap()
        .create_token(access.space, access.user_id, &name, role)
    {
        Ok(token) => Json(serde_json::json!({ "token": token })).into_response(),
        Err(error) => failed(error),
    }
}

async fn revoke_token(
    access: Access,
    State(store): State<Shared>,
    Path(id): Path<i64>,
) -> Response {
    match store.lock().unwrap().revoke_token(access.space, id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such token").into_response(),
        Err(error) => failed(error),
    }
}

// ---- joining, choosing a space, one's own account ---------------------------------------

/// The session behind a request on a route the guard lets through unsigned (`/invite/…`).
fn session_of(store: &crate::store::Store, headers: &HeaderMap) -> Option<Signed> {
    let token = crate::auth::cookie(headers, crate::auth::COOKIE)?;
    store.session(&token).ok().flatten()
}

fn plain_page(title: &str, body: &str) -> Html<String> {
    Html(crate::auth::auth_page(title, body))
}

async fn invite_page(
    State(store): State<Shared>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let store = store.lock().unwrap();
    let invite = match store.invite(&token) {
        Ok(invite) => invite,
        Err(error) => return failed(error),
    };
    let Some(invite) = invite.filter(|i| i.open()) else {
        return (
            StatusCode::NOT_FOUND,
            plain_page("Invite link", r#"<p>This link is not (or no longer) valid: used, revoked, or more than a week old. Ask for a new one.</p>"#),
        )
            .into_response();
    };
    let into = match invite.whole_organisation {
        true => store
            .organisation_of(invite.space)
            .and_then(|o| store.organisation_name(o))
            .unwrap_or_default(),
        false => invite.space_name.clone(),
    };
    let what = format!(
        "<p><b>{by}</b> invites you into <b>{into}</b> as <b>{role}</b>.</p>",
        by = escape(&invite.by),
        into = escape(&into),
        role = invite.role.as_str()
    );
    let body = match session_of(&store, &headers) {
        Some(signed) => format!(
            r#"{what}<form method="post"><button>Join as {name}</button></form><p style="margin-top:14px"><small>Not you? <a href="/login?next=/invite/{token}">Sign in as someone else</a>.</small></p>"#,
            name = escape(&signed.display_name),
            token = escape(&token),
        ),
        None => format!(
            r#"{what}<form method="post">
<label for="display_name">Your name</label><input id="display_name" name="display_name" maxlength="60" autocomplete="name" required autofocus>
<label for="name">Sign-in name</label><input id="name" name="name" maxlength="40" autocomplete="username" autocapitalize="none" required>
<small>Letters, digits, dots, dashes or underscores.</small>
<label for="password">Password</label><input id="password" name="password" type="password" minlength="{min}" autocomplete="new-password" required>
<label for="again">Password again</label><input id="again" name="again" type="password" minlength="{min}" autocomplete="new-password" required>
<button>Make my account and join</button></form>
<p style="margin-top:14px"><small>Have an account here? <a href="/login?next=/invite/{token}">Sign in</a> first.</small></p>"#,
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
    State(store): State<Shared>,
    Path(token): Path<String>,
    Extension(Client(client)): Extension<Client>,
    headers: HeaderMap,
    Form(form): Form<JoinForm>,
) -> Response {
    if !from_this_site(&Method::POST, &headers) {
        return refused("refused: this came from another site");
    }
    let (invite, signed) = {
        let store = store.lock().unwrap();
        (
            store.invite(&token).ok().flatten().filter(|i| i.open()),
            session_of(&store, &headers),
        )
    };
    let Some(invite) = invite else {
        return (
            StatusCode::NOT_FOUND,
            "this link is not (or no longer) valid",
        )
            .into_response();
    };
    // Signed in already: in, and the session goes to the space.
    if let Some(signed) = signed {
        let store = store.lock().unwrap();
        return match store.use_invite(&invite, signed.user_id) {
            Ok(true) => {
                let _ = store.set_session_space(signed.session_id, invite.space);
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
    } else if store
        .lock()
        .unwrap()
        .user_id(&name)
        .ok()
        .flatten()
        .is_some()
    {
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
                    r#"<p class="problem" role="alert">{}</p><p><a href="/invite/{}">Back</a></p>"#,
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
    let store = store.lock().unwrap();
    let made = (|| -> rusqlite::Result<Option<String>> {
        let user = store.create_user(&name, &display_name, &hash, false)?;
        if !store.use_invite(&invite, user)? {
            return Ok(None);
        }
        store
            .create_session(user, Some(invite.space), user_agent, &client)
            .map(Some)
    })();
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
async fn welcome(State(store): State<Shared>, Extension(signed): Extension<Signed>) -> Response {
    let store = store.lock().unwrap();
    if store.spaces_of(signed.user_id).is_ok_and(|s| !s.is_empty()) {
        return Redirect::to("/").into_response();
    }
    let make = if signed.instance_admin || store.open_spaces() {
        r#"<p>Or start your own:</p><form method="post" action="/spaces"><label for="name">Name of your organisation</label><input id="name" name="name" maxlength="60" required placeholder="Fablab Zuid"><button>Make it</button></form>"#
    } else {
        ""
    };
    plain_page(
        "Welcome",
        &format!(
            r#"<p>Hello {name}. You are not in a space yet: ask someone in one for an invite link.</p>{make}<form method="post" action="/logout" style="margin-top:18px"><button class="ghost" style="background:transparent;color:var(--ink);border:1px solid var(--line)">Sign out</button></form>"#,
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
    State(store): State<Shared>,
    Extension(signed): Extension<Signed>,
    Form(form): Form<SwitchForm>,
) -> Response {
    let store = store.lock().unwrap();
    match store.role_of(signed.user_id, form.space) {
        Ok(Some(_)) => match store.set_session_space(signed.session_id, form.space) {
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
    State(store): State<Shared>,
    Extension(signed): Extension<Signed>,
    access: Option<Extension<Access>>,
    headers: HeaderMap,
    Form(form): Form<NewSpaceForm>,
) -> Response {
    let name = match clean_name(&form.name) {
        Ok(name) => name,
        Err(r) => return *r,
    };
    let store = store.lock().unwrap();
    let made = (|| -> rusqlite::Result<Result<i64, &'static str>> {
        if form.organisation.as_deref() == Some("this") {
            let Some(Extension(access)) = &access else {
                return Ok(Err("not in a space"));
            };
            let organisation = store.organisation_of(access.space)?;
            if store
                .org_role_of(signed.user_id, organisation)?
                .is_none_or(|r| r < Role::Admin)
            {
                return Ok(Err("only an admin of the organisation makes spaces in it"));
            }
            let space = store.create_space(organisation, &name)?;
            let _ = store.audit(access, "made a space", &name, "");
            Ok(Ok(space))
        } else {
            if !(signed.instance_admin || store.open_spaces()) {
                return Ok(Err("only who runs the server makes organisations here"));
            }
            Ok(Ok(store
                .create_organisation(&name, &name, signed.user_id)?
                .1))
        }
    })();
    match made {
        Ok(Ok(space)) => {
            let _ = store.set_session_space(signed.session_id, space);
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
    State(store): State<Shared>,
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
    let hash = match store.lock().unwrap().credentials(&signed.name) {
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
    let store = store.lock().unwrap();
    // A new password ends every session; this one is made again.
    let done = store
        .set_password(signed.user_id, &new_hash)
        .and_then(|()| {
            let space = store.spaces_of(signed.user_id)?.first().map(|s| s.id);
            store.create_session(signed.user_id, space, &user_agent, &client)
        });
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

async fn open_spaces(State(store): State<Shared>, Form(form): Form<OpenForm>) -> Response {
    match store.lock().unwrap().set_open_spaces(form.open.is_some()) {
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
