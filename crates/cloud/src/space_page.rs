//! A space's own page (its members, invite links, who may use which machine, API tokens,
//! the audit log), joining by an invite link, choosing or making a space, and one's own
//! password. What each needs is in [`crate::access::RULES`]; the rules within (no role
//! above one's own, an organisation keeps an owner) are here.

use crate::access::{Access, Level, Role};
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
        .route("/machines/{id}/access", post(machine_access))
        .route("/machines/{id}/grants", post(grant))
        .route("/machines/{id}/grants/{user}/remove", post(ungrant))
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

fn day(at: i64) -> String {
    crate::spoolman::iso(at).chars().take(10).collect()
}

const SPACE_STYLE: &str = r#"
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
  .machine-access { border-top: 1px solid var(--line); padding: 12px 0; }
  .machine-access:first-of-type { border-top: 0; }
  .machine-access header { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
  .machine-access .grants { margin: 8px 0 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 6px; }
  .machine-access .grants li { display: inline-flex; align-items: center; gap: 6px; padding: 3px 4px 3px 10px; border: 1px solid var(--line); border-radius: 999px; font-size: 13px; }
  .machine-access .grants form { margin: 0; }
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

        // Members.
        let members = store.members(space)?;
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
                r#"<tr><td><b>{display}</b>{you}<br><small>{name}</small></td><td>{org}</td><td>{own}</td><td><small>{since}</small></td><td>{remove}</td></tr>"#,
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
        let members_html = format!(
            r#"<section class="section"><h3>Members</h3><p class="sub">A role in the organisation counts in all of its spaces; a role in this space only here. The higher one counts. Owners and admins manage the space and every machine; members use the machines as far as their inductions go; guests look only.</p>
<table class="list"><thead><tr><th>Who</th><th>Organisation</th><th>This space</th><th>Since</th><th></th></tr></thead><tbody>{rows}</tbody></table></section>"#
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

        // Machines and inductions.
        let machines = store.machines(space)?;
        let mut machine_html = String::new();
        for machine in &machines {
            let induction = store.induction_required(space, &machine.id)?;
            let grants = store.grants(space, &machine.id)?;
            let mut chips = String::new();
            for g in &grants {
                let ended = g.expires_at.is_some_and(|t| t <= crate::store::now());
                chips += &format!(
                    r#"<li>{who} · {level}{until}{remove}</li>"#,
                    who = escape(&g.display_name),
                    level = g.level.as_str(),
                    until = g.expires_at.map_or(String::new(), |t| format!(
                        r#" <small>{} {}</small>"#,
                        if ended { "ended" } else { "until" },
                        day(t)
                    )),
                    remove = if admin {
                        format!(
                            r#"<form method="post" action="machines/{m}/grants/{u}/remove" data-async data-then="reload" data-done="Taken away"><button title="Take away" aria-label="Take away">×</button></form>"#,
                            m = escape(&machine.id),
                            u = g.user_id
                        )
                    } else {
                        String::new()
                    },
                );
            }
            let candidates: String = members
                .iter()
                .filter(|m| m.role == Role::Member)
                .map(|m| {
                    format!(
                        r#"<option value="{}">{}</option>"#,
                        m.user_id,
                        escape(&m.display_name)
                    )
                })
                .collect();
            let controls = if admin {
                format!(
                    r#"<form method="post" action="machines/{id}/access" data-async data-then="reload" data-done="Saved"><label><input type="checkbox" name="induction" value="1"{checked} onchange="this.form.requestSubmit()"> Needs an induction</label></form>"#,
                    id = escape(&machine.id),
                    checked = if induction { " checked" } else { "" }
                )
            } else {
                format!(
                    r#"<small>{}</small>"#,
                    if induction {
                        "Needs an induction"
                    } else {
                        "Open to members"
                    }
                )
            };
            let add = if admin && induction {
                if candidates.is_empty() {
                    r#"<p class="muted" style="margin:8px 0 0;font-size:13px">No members to give one to (owners and admins need none).</p>"#.to_owned()
                } else {
                    format!(
                        r#"<form method="post" action="machines/{id}/grants" data-async data-then="reload" data-done="Induction given" class="switch" style="margin:8px 0 0"><select class="small" name="user" aria-label="Who">{candidates}</select><select class="small" name="level" aria-label="May"><option value="use">use (jobs, labels)</option><option value="operate">operate (also move, heat, G-code)</option></select><label class="muted" style="font-size:13px">until <input class="small" type="date" name="until"></label><button class="small">Give</button></form>"#,
                        id = escape(&machine.id)
                    )
                }
            } else {
                String::new()
            };
            machine_html += &format!(
                r#"<div class="machine-access"><header><b>{name}</b>{controls}</header>{list}{add}</div>"#,
                name = escape(&machine.name),
                list = if induction && !chips.is_empty() {
                    format!(r#"<ul class="grants">{chips}</ul>"#)
                } else if induction {
                    r#"<p class="muted" style="margin:6px 0 0;font-size:13px">Nobody has had the induction yet.</p>"#.to_owned()
                } else {
                    String::new()
                },
            );
        }
        let machines_section = format!(
            r#"<section class="section"><h3>Who may use which machine</h3><p class="sub">A machine open to members: every member may run jobs on it and operate it. One that needs an induction (a laser, say): only who had it, as far as it goes, until its end if it has one.</p>{}</section>"#,
            if machine_html.is_empty() {
                r#"<p class="muted">No machines yet.</p>"#.to_owned()
            } else {
                machine_html
            }
        );

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
                    r#"<li><time>{at}</time><b>{actor}</b> {action} {target}</li>"#,
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
  {org_html}
  {members_html}
  {invites_html}
  {machines_section}
  {tokens_html}
  {audit_html}
  {account_html}
  {instance_html}
</main>
<script>
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
    let on = form.induction.is_some();
    match store
        .lock()
        .unwrap()
        .set_induction_required(access.space, &machine, on)
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
    let Some(level) =
        Level::parse(&form.level).filter(|l| matches!(l, Level::Use | Level::Operate))
    else {
        return (StatusCode::BAD_REQUEST, "use or operate").into_response();
    };
    let until = match form
        .until
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        None => None,
        Some(date) => match end_of_day(date) {
            Some(at) => Some(at),
            None => return (StatusCode::BAD_REQUEST, "a date as 2026-12-31").into_response(),
        },
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

/// The end of a day (`2026-12-31`, UTC), as Unix seconds.
fn end_of_day(date: &str) -> Option<i64> {
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
        Ok(false) => (StatusCode::NOT_FOUND, "no such induction").into_response(),
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
    use super::end_of_day;

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
