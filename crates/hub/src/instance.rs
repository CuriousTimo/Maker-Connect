//! Instance: what spans the hub, for its instance admins (`docs/design/ui.md` "Instance",
//! mockup `docs/design/ui/mockups/instance.html`). Today its agents: every agent with its release,
//! computer, connection, last seen and devices, its devices grouped by the `agent_id` they send
//! (`docs/design/prusa-sdk-extension.md` part 6a).

use crate::access::Access;
use crate::pages::machine_page::ago;
use crate::store::AgentOfHub;
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Json, Router,
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::get,
};

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/instance", get(page))
        .route("/api/v1/instance/agents", get(agents))
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
        r#"<tr><td><b>{computer}</b><br><small class="muted" title="{id}">id {short}</small></td><td>{release}</td><td><span class="st {class}">{state}</span></td><td>{seen}</td><td>{devices}</td></tr>"#,
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
    )
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
<p class="muted">Each agent is a computer beside the machines, running the Maker Connect agent; its devices are grouped by the id the agent sends.</p>
{body}
<p><small>An agent from before agents sent their id is not listed here; its devices are on their own pages.</small></p></div>"#,
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
{card}
</main>
</body>
</html>
"##,
        header = header(&space, "", "instance"),
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

    #[test]
    fn an_agent_row_says_its_computer_release_state_and_devices() {
        let row = row(&agent(true));
        assert!(row.contains("<b>orangepipc</b>"), "{row}");
        assert!(row.contains("id 3f9a…c21e"), "{row}");
        assert!(row.contains("<td>v0.3.0</td>"), "{row}");
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
