//! Instance: what spans the hub, for its instance admins (`docs/design/ui.md` "Instance",
//! mockup `docs/design/ui/mockups/instance.html`). Today its agents: every agent with its release,
//! computer, connection, last seen and devices, its devices grouped by the `agent_id` they send
//! (`docs/design/prusa-sdk-extension.md` part 6a).

use crate::access::Access;
use crate::pages::machine_page::ago;
use crate::store::{AgentOfHub, InstanceDevice};
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/instance", get(page))
        .route("/api/v1/instance/agents", get(agents))
        .route("/instance/devices/{id}/space", post(to_space))
}

/// `POST /instance/devices/{id}/space`: a waiting device into a space (what pairing with its code
/// there does), or one in a space moved to another (the machine page's move). An instance admin's
/// (the route's gate). Wiki Design: Devices and Rights, "Instance: every device, per space".
async fn to_space(
    access: Access,
    State(store): State<Store>,
    Path(id): Path<String>,
    Form(form): Form<crate::web::machines::MoveForm>,
) -> Response {
    let to = match store.space_by_uid(form.space.trim()).await {
        Ok(Some(to)) => to,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such space").into_response(),
        Err(error) => return failed(error),
    };
    let (name, from, code) = match store.space_of_device(&id).await {
        Ok(Some(found)) => found,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such device").into_response(),
        Err(error) => return failed(error),
    };
    if let Some(from) = from {
        return crate::web::machines::moved(&access, &store, (from, to), &id, &name).await;
    }
    let Some(code) = code else {
        return (StatusCode::CONFLICT, "it waits without a code").into_response();
    };
    match store.pair(to, &code).await {
        Ok(Some(_)) => {
            let there = Access {
                space: to,
                ..access.clone()
            };
            let _ = store
                .audit(&there, "added a machine", &name, "on the Instance page")
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        // Another pairing took it first.
        Ok(None) => (StatusCode::CONFLICT, "it is in a space already").into_response(),
        Err(error) => failed(error),
    }
}

/// `GET /api/v1/instance/agents`.
async fn agents(State(store): State<Store>) -> Response {
    match store.agents().await {
        Ok(agents) => Json(agents).into_response(),
        Err(error) => failed(error),
    }
}

/// When it was last seen, as the table says it.
fn seen(online: bool, seconds: Option<i64>) -> String {
    match seconds {
        _ if online => "now".to_owned(),
        Some(s) => ago(s),
        None => "never".to_owned(),
    }
}

/// The agent's own page's port (the agent's `[web]` default; `add_machine_page` names it too).
const AGENT_PORT: u16 = 8470;

/// One agent's row.
fn row(agent: &AgentOfHub) -> String {
    let short = format!(
        "{}…{}",
        &agent.id[..4.min(agent.id.len())],
        &agent.id[agent.id.len().saturating_sub(4)..]
    );
    let (state, class) = if agent.online {
        ("Connected", "wait")
    } else {
        ("Not connected", "warn")
    };
    let devices: Vec<String> = agent
        .devices
        .iter()
        .map(|d| {
            let mut notes: Vec<String> = Vec::new();
            notes.push(
                d.space
                    .as_deref()
                    .map_or("not added yet".to_owned(), escape),
            );
            if agent.online && !d.online {
                notes.push("not connected".to_owned());
            }
            let name = if d.space.is_some() {
                format!(
                    r#"<a href="machines/{}">{}</a>"#,
                    escape(&d.id),
                    escape(&d.name)
                )
            } else {
                escape(&d.name)
            };
            format!("{name} <small>({})</small>", notes.join(", "))
        })
        .collect();
    format!(
        r#"<tr><td><b>{computer}</b><br><small class="muted" title="{id}">id {short}</small>{page}</td><td>{release}</td><td><span class="st {class}">{state}</span></td><td>{seen}</td><td>{devices}</td></tr>"#,
        computer = agent
            .computer
            .as_deref()
            .map_or("A computer without a name".to_owned(), escape),
        id = escape(&agent.id),
        short = escape(&short),
        release = agent
            .release
            .as_deref()
            .map(|r| escape(r.strip_prefix("Maker Connect agent ").unwrap_or(r)))
            .unwrap_or_else(|| "—".to_owned()),
        seen = seen(agent.online, agent.seen_seconds_ago),
        devices = devices.join("<br>"),
        // Its own page, on the computer beside the machines (same network as the browser).
        page = agent
            .address
            .as_deref()
            .map(|a| format!(
                r#"<br><a href="{url}" target="_blank" rel="noopener">Its page ↗</a>"#,
                url = escape(&crate::access_page::device_url(a, Some(AGENT_PORT)))
            ))
            .unwrap_or_default(),
    )
}

/// What a device is, for its row.
fn kind_label(device: &InstanceDevice) -> &'static str {
    use maker_connect_protocol::MachineKind;
    match device.kind {
        Some(MachineKind::DoorController) => "Door controller",
        Some(MachineKind::Cameras) => "Cameras",
        Some(kind) => kind.label(),
        None => "Unknown",
    }
}

/// One device's row: what it is, how it connects, and the one select to add or move it.
fn device_row(device: &InstanceDevice, spaces: &[(i64, String, String)]) -> String {
    let (state, class) = if device.online {
        ("Connected", "wait")
    } else {
        ("Not connected", "warn")
    };
    let options: String = spaces
        .iter()
        .filter(|(id, ..)| Some(*id) != device.space)
        .map(|(_, uid, label)| {
            format!(
                r#"<option value="{}">{}</option>"#,
                escape(uid),
                escape(label)
            )
        })
        .collect();
    let (button, confirm) = if device.space.is_some() {
        (
            "Move",
            "Move it to that space? Its jobs, permissions and loaded spool stay in this one. Not while it has work under way.",
        )
    } else {
        ("Add to space", "Add it to that space?")
    };
    let action = if options.is_empty() {
        String::new()
    } else {
        format!(
            r#"<form method="post" action="instance/devices/{id}/space" data-async data-done="Done" data-then="reload" data-confirm="{confirm}" class="switch"><select name="space" aria-label="To which space" required>{options}</select><button class="ghost">{button}</button></form>"#,
            id = escape(&device.id),
        )
    };
    let code = device
        .code
        .as_deref()
        .map(|c| format!(r#"<br><small class="muted">Code {}</small>"#, escape(c)))
        .unwrap_or_default();
    format!(
        r#"<tr><td><b>{name}</b>{code}</td><td>{kind}</td><td>{via}</td><td><span class="st {class}">{state}</span></td><td>{seen}</td><td>{action}</td></tr>"#,
        name = escape(&device.name),
        kind = kind_label(device),
        via = device.agent.as_deref().map_or("Direct".to_owned(), escape),
        seen = seen(device.online, device.seen_seconds_ago),
    )
}

/// The devices, a card per space: those waiting for one first, then every space, also an empty one.
fn device_cards(devices: &[InstanceDevice], spaces: &[(i64, String, String)]) -> String {
    let table = |rows: String| {
        format!(
            r#"<div class="table-wrap dense"><table><thead><tr><th>Device</th><th>Type</th><th>Through</th><th>State</th><th>Last seen</th><th>Space</th></tr></thead><tbody>{rows}</tbody></table></div>"#
        )
    };
    let rows = |space: Option<i64>| -> String {
        devices
            .iter()
            .filter(|d| d.space == space)
            .map(|d| device_row(d, spaces))
            .collect()
    };
    let mut out = String::new();
    let waiting = rows(None);
    if !waiting.is_empty() {
        out.push_str(&format!(
            r#"<div class="card"><div class="head"><h2>Waiting for a space</h2></div>
<p class="muted">These have registered and are in no space yet. Check the code against the one the device shows.</p>{}</div>
"#,
            table(waiting)
        ));
    }
    for (id, _, label) in spaces {
        let these = rows(Some(*id));
        let body = if these.is_empty() {
            r#"<p class="muted">No devices.</p>"#.to_owned()
        } else {
            table(these)
        };
        out.push_str(&format!(
            r#"<div class="card"><div class="head"><h2>{}</h2></div>
{body}</div>
"#,
            escape(label)
        ));
    }
    out
}

/// The Agents card.
fn agents_card(agents: &[AgentOfHub]) -> String {
    let count: usize = agents.iter().map(|a| a.devices.len()).sum();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let body = if agents.is_empty() {
        r#"<p>No agent has reported yet. An agent is a computer beside the machines, running the Maker Connect agent; its devices then appear here.</p>"#.to_owned()
    } else {
        format!(
            r#"<div class="table-wrap dense"><table><thead><tr><th>Agent</th><th>Release</th><th>State</th><th>Last seen</th><th>Devices</th></tr></thead><tbody>{}</tbody></table></div>"#,
            agents.iter().map(row).collect::<String>()
        )
    };
    format!(
        r#"<div class="card"><div class="head"><h2>Agents</h2><span class="muted">{agents} · {devices}</span></div>
{body}</div>"#,
        agents = plural(agents.len(), "agent", "agents"),
        devices = plural(count, "device", "devices"),
    )
}

async fn page(access: Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    let agents = match store.agents().await {
        Ok(agents) => agents,
        Err(error) => return failed(error),
    };
    let devices = match store.instance_devices().await {
        Ok(devices) => devices,
        Err(error) => return failed(error),
    };
    let spaces = match store.all_spaces().await {
        Ok(spaces) => spaces,
        Err(error) => return failed(error),
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Instance · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1">
<div class="head"><h1>Instance</h1></div>
{devices}
{card}
</main>
</body>
</html>
"##,
        header = header(&space, "", "instance"),
        devices = device_cards(&devices, &spaces),
        card = agents_card(&agents),
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::AgentDevice;

    fn agent(online: bool) -> AgentOfHub {
        AgentOfHub {
            id: "3f9a000000000000000000000000c21e".into(),
            release: Some("Maker Connect agent v0.3.0".into()),
            computer: Some("orangepipc".into()),
            online,
            seen_seconds_ago: Some(3 * 86_400),
            address: Some("192.168.60.67".into()),
            devices: vec![
                AgentDevice {
                    id: "m1".into(),
                    name: "Labels-1".into(),
                    space: Some("Workshop North".into()),
                    online,
                    seen_seconds_ago: Some(0),
                    extensions: vec![],
                },
                AgentDevice {
                    id: "m2".into(),
                    name: "Tape-1".into(),
                    space: None,
                    online: false,
                    seen_seconds_ago: None,
                    extensions: vec![],
                },
            ],
        }
    }

    /// Two devices whose `INFO` names one agent are one agent; a Prusa (no `agent_id`) is not one.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn devices_are_grouped_by_their_agent(pool: sqlx::PgPool) {
        use maker_connect_protocol::MachineKind;
        let store = Store::for_test(&pool).await;
        let id = "0123456789abcdef0123456789abcdef";
        // A Marlin's `firmware` is its own (part 9): the release is the request's
        // `User-Agent-Version`, kept beside it (part 6a).
        let info = |extensions: serde_json::Value| {
            let mut details = crate::devices::prusa_connect::to_details(&serde_json::json!({
                "firmware": "Marlin 2.0.9.3", "agent_id": id, "extensions": extensions,
                "network_info": {"hostname": "orangepipc"},
            }));
            details.agent_release = Some("v0.3.0".to_owned());
            details
        };
        let (_, lw, lw_token) = store
            .register_test_machine("LW450", MachineKind::Label)
            .await;
        let (_, pnp, pnp_token) = store.register_test_machine("PnP", MachineKind::Label).await;
        let (_, _, mini_token) = store.register_test_machine("MINI", MachineKind::Fdm).await;
        assert!(
            store
                .set_details(&lw_token, &info(serde_json::json!(["label.page_size"])))
                .await
                .unwrap()
        );
        assert!(
            store
                .set_details(&pnp_token, &info(serde_json::json!([])))
                .await
                .unwrap()
        );
        let prusa = crate::devices::prusa_connect::to_details(
            &serde_json::json!({"firmware": "6.3.0", "agent_id": "not an id"}),
        );
        assert!(store.set_details(&mini_token, &prusa).await.unwrap());
        let agents = store.agents().await.unwrap();
        assert_eq!(agents.len(), 1, "{agents:?}");
        let agent = &agents[0];
        assert_eq!(agent.id, id);
        assert_eq!(agent.release.as_deref(), Some("v0.3.0"));
        assert_eq!(agent.computer.as_deref(), Some("orangepipc"));
        let ids: Vec<&str> = agent.devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, [lw.as_str(), pnp.as_str()]);
        assert_eq!(agent.devices[0].extensions, ["label.page_size"]);
        assert!(agent.devices[0].space.is_some());
    }

    /// A device waiting shows its code and Add to space; one in a space Move, without its own space
    /// among the choices; a space without devices says so.
    #[test]
    fn devices_are_listed_per_space_waiting_first() {
        use maker_connect_protocol::MachineKind;
        let device = |id: &str, space: Option<i64>, code: Option<&str>| InstanceDevice {
            id: id.into(),
            name: format!("{id}-name"),
            kind: Some(MachineKind::DoorController),
            space,
            code: code.map(Into::into),
            agent: None,
            online: false,
            seen_seconds_ago: None,
        };
        let spaces = vec![
            (1, "u1".to_owned(), "Org · North".to_owned()),
            (2, "u2".to_owned(), "Org · South".to_owned()),
        ];
        let html = device_cards(
            &[
                device("d1", None, Some("ABC123")),
                device("d2", Some(1), None),
            ],
            &spaces,
        );
        let waiting = html.find("Waiting for a space").unwrap();
        let north = html.find("Org · North</h2>").unwrap();
        let south = html.find("Org · South</h2>").unwrap();
        assert!(waiting < north && north < south, "{html}");
        assert!(html[waiting..north].contains("Code ABC123"), "{html}");
        assert!(
            html[waiting..north].contains(">Add to space</button>"),
            "{html}"
        );
        assert!(html[waiting..north].contains("<td>Door controller</td><td>Direct</td>"));
        let d2 = &html[north..south];
        assert!(d2.contains(">Move</button>"), "{d2}");
        assert!(
            !d2.contains(r#"value="u1""#) && d2.contains(r#"value="u2""#),
            "{d2}"
        );
        assert!(html[south..].contains("No devices."), "{html}");
        // Nothing waiting: no such card.
        assert!(!device_cards(&[device("d2", Some(1), None)], &spaces).contains("Waiting"));
    }

    /// A registered device waits with its code; once paired it is in that space, its code gone.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn every_device_is_listed_waiting_or_in_its_space(pool: sqlx::PgPool) {
        use maker_connect_protocol::MachineKind;
        let store = Store::for_test(&pool).await;
        let (id, _, code) = store
            .register_linked(
                "Door",
                MachineKind::DoorController,
                "Door controller firmware 0.1.3",
                "prusa-connect",
            )
            .await
            .unwrap();
        let find = |devices: Vec<InstanceDevice>| devices.into_iter().find(|d| d.id == id).unwrap();
        let waiting = find(store.instance_devices().await.unwrap());
        assert_eq!(waiting.space, None);
        assert_eq!(waiting.code.as_deref(), Some(code.as_str()));
        assert_eq!(waiting.kind, Some(MachineKind::DoorController));
        assert_eq!(
            store.space_of_device(&id).await.unwrap(),
            Some(("Door".to_owned(), None, Some(code.clone())))
        );
        let spaces = store.all_spaces().await.unwrap();
        let space = spaces[0].0;
        assert!(store.pair(space, &code).await.unwrap().is_some());
        let paired = find(store.instance_devices().await.unwrap());
        assert_eq!(paired.space, Some(space));
        assert_eq!(paired.code, None, "a device in a space shows no code");
    }

    #[test]
    fn an_agent_row_says_its_computer_release_state_and_devices() {
        let row = row(&agent(true));
        assert!(row.contains("<b>orangepipc</b>"), "{row}");
        assert!(row.contains("id 3f9a…c21e"), "{row}");
        assert!(row.contains("<td>v0.3.0</td>"), "{row}");
        // Its own page, in a new tab, at the address it reached the hub from.
        assert!(
            row.contains(r#"<a href="http://192.168.60.67:8470/" target="_blank" rel="noopener">Its page ↗</a>"#),
            "{row}"
        );
        assert!(row.contains(r#"<span class="st wait">Connected</span></td><td>now</td>"#));
        assert!(row.contains(
            r#"<a href="machines/m1">Labels-1</a> <small>(Workshop North)</small><br>Tape-1 <small>(not added yet, not connected)</small>"#
        ), "{row}");
        let row = super::row(&agent(false));
        assert!(
            row.contains(r#"Not connected</span></td><td>3 days ago</td>"#),
            "{row}"
        );
        assert!(agents_card(&[agent(true)]).contains("1 agent · 2 devices"));
        assert!(agents_card(&[]).contains("No agent has reported yet"));
    }
}
