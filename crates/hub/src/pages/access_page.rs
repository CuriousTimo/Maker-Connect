//! Access: getting in, physical access, a thing of its own (machine use is crate::permissions_page).
//! Today: the space's door controllers (`docs/design/prusa-sdk-extension.md` part 7) and their locks,
//! opened by a Space admin. Who may open which door, badges and the door log are not built yet.

/// How long an `OPEN_LOCK` may still run after Open was pressed (`expires_at`, part 7; owner,
/// 2026-11-27: "ja 5s"). The doors spring open by themselves: a late open would leave one open.
pub(crate) const OPEN_EXPIRES_S: u64 = 5;

/// The build for door controllers this hub carries (part 7 "Updating the firmware"): an ESP-IDF app
/// image beside the hub's files folder, `door-controller-firmware.bin`. None: no update offered.
pub(crate) fn firmware_build(store: &crate::Store) -> Option<std::path::PathBuf> {
    let path = store
        .files_dir()
        .parent()?
        .join("door-controller-firmware.bin");
    path.is_file().then_some(path)
}

/// The version an ESP-IDF app image says of itself: `esp_app_desc_t.version` (32 bytes from offset
/// 16 of the descriptor, which follows the 24-byte image header and the first 8-byte segment header;
/// magic `0xABCD5432`).
pub(crate) fn build_version(image: &[u8]) -> Option<String> {
    let desc = image.get(32..32 + 48)?;
    if desc[..4] != 0xABCD_5432u32.to_le_bytes() {
        return None;
    }
    let version = &desc[16..48];
    let end = version.iter().position(|&b| b == 0).unwrap_or(32);
    std::str::from_utf8(&version[..end]).ok().map(str::to_owned)
}

/// The door controllers of the space (part 7) and their locks, each with its Open button. Nothing
/// else: no example rows (owner, 2026-11-27: "al die demo troep").
pub(crate) async fn tab(store: &crate::Store, space: i64) -> String {
    let boards = match store.door_controllers(space).await {
        Ok(boards) => boards,
        Err(error) => {
            return format!(
                r#"<section class="section"><p class="muted">Door controllers could not be read: {}</p></section>"#,
                crate::escape(&error.to_string()),
            );
        }
    };
    let mut html = String::new();
    for board in &boards {
        html.push_str(&board_card(store, board));
    }
    if boards.is_empty() {
        html.push_str(r#"<section class="section"><div class="a-head"><h3>Door controllers</h3></div><p class="muted">None yet. A door controller is added like a machine: its setup page shows a code, typed in under Add machine.</p></section>"#);
    }
    format!("<div data-access-live>{html}</div>{LIVE}")
}

/// Live: on every change of the space's machines (the machine stream, as the Now page), the tab
/// is read again and put in place, so a lock's state and an Open's outcome show without a reload.
const LIVE: &str = r#"<script>
  (() => {
    const box = document.querySelector("[data-access-live]");
    let soon = null,
      last = box.innerHTML;
    const refresh = async () => {
      try {
        const r = await fetch("space/access", { headers: { "x-requested-with": "fetch" } });
        if (!r.ok) return;
        const fresh = new DOMParser().parseFromString(await r.text(), "text/html").querySelector("[data-access-live]");
        // Only what changed on the hub: the same answer again would undo the countdown below.
        if (fresh && fresh.innerHTML !== last) box.innerHTML = last = fresh.innerHTML;
      } catch {}
    };
    // The board reports its rest every few seconds; between reports the page counts down itself.
    setInterval(() => {
      for (const el of box.querySelectorAll("[data-rest]")) {
        const left = Math.max(0, Number(el.dataset.rest) - 1);
        el.dataset.rest = left;
        el.textContent = left;
      }
    }, 1000);
    new EventSource("api/machines/stream").onmessage = () => {
      // At most one read per 200 ms, however fast the changes come.
      soon ??= setTimeout(() => {
        soon = null;
        refresh();
      }, 200);
    };
  })();
</script>"#;

fn board_card(store: &crate::Store, board: &crate::store::Machine) -> String {
    use crate::escape;
    use maker_connect_protocol::DoorReading;
    let linked = store.lock().unwrap().linked(&board.id);
    let telemetry = board.telemetry.as_ref();
    let firmware = telemetry.and_then(|t| t.firmware.as_deref()).unwrap_or("–");
    let link = if linked {
        r#"<span class="a-state ok">connected</span>"#
    } else {
        r#"<span class="a-state warn">not connected</span>"#
    };
    let locks = telemetry.and_then(|t| t.locks.clone()).unwrap_or_default();
    // The build this hub carries, offered while the board is there and idle (part 6b's guard).
    let build = firmware_build(store)
        .and_then(|p| std::fs::read(p).ok())
        .map(|image| build_version(&image).unwrap_or_else(|| "unknown version".to_owned()));
    let idle = telemetry.is_some_and(|t| t.state == maker_connect_protocol::MachineState::Idle);
    let update = match &build {
        Some(version) if linked && idle => format!(
            r#"<form method="post" action="space/access/update" data-async data-done="Updating: the board restarts" class="row"><input type="hidden" name="machine" value="{id}"><span class="muted">Build on this hub: <b>{v}</b></span><span class="fw"><button class="btn small">Update firmware</button></span></form>"#,
            id = escape(&board.id),
            v = escape(version)
        ),
        Some(version) => format!(
            r#"<p class="muted">Build on this hub: <b>{}</b>. Update when the board is connected and idle.</p>"#,
            escape(version)
        ),
        None => String::new(),
    };
    let mut rows = String::new();
    for lock in &locks {
        let n = lock.lock;
        let shown = n + 1;
        let (class, now) = match lock.door {
            None => ("bad", "input does not answer".to_owned()),
            Some(DoorReading::Closed) => ("ok", "switch closed".to_owned()),
            Some(DoorReading::Open) => ("warn", "switch open, or no switch".to_owned()),
        };
        let rest = if lock.resting_s > 0 {
            format!(
                r#" · resting <span data-rest="{0}">{0}</span> s"#,
                lock.resting_s
            )
        } else {
            String::new()
        };
        let button = if linked {
            format!(
                r#"<form method="post" action="space/access/open" data-async data-done="Sent"><input type="hidden" name="machine" value="{id}"><input type="hidden" name="lock" value="{n}"><span class="fw"><button class="btn small primary">Open</button></span></form>"#,
                id = escape(&board.id)
            )
        } else {
            r#"<span class="fw"><button class="btn small" disabled title="The board is not connected">Open</button></span>"#.to_owned()
        };
        rows.push_str(&format!(
            r#"<tr><td><b>Lock {shown}</b><br><small>relay {shown} · input {shown}</small></td><td><span class="a-state {class}">{now}</span><small>{rest}</small></td><td>{button}</td></tr>"#
        ));
    }
    if locks.is_empty() {
        rows.push_str(r#"<tr><td colspan="3" class="muted">No locks reported yet.</td></tr>"#);
    }
    // The last Opens and how they ended: "Lock resting", "Expired" as the board says them.
    let last: String = board
        .commands
        .iter()
        .filter(|c| c.label.starts_with("Open lock") || c.label == UPDATE_LABEL)
        .take(5)
        .map(|c| {
            let how = match c.state {
                crate::store::CommandState::Queued => "waiting".to_owned(),
                crate::store::CommandState::Sent => "sent".to_owned(),
                crate::store::CommandState::Accepted => "opening".to_owned(),
                crate::store::CommandState::Finished => "opened".to_owned(),
                crate::store::CommandState::Rejected => {
                    format!("not opened: {}", c.reason.as_deref().unwrap_or("refused"))
                }
            };
            format!("<li><b>{}</b> {}</li>", escape(&c.label), escape(&how))
        })
        .collect();
    let last = if last.is_empty() {
        String::new()
    } else {
        format!(r#"<ul class="a-log">{last}</ul>"#)
    };
    format!(
        r#"<section class="section"><div class="a-head"><h3>{name}</h3>{link}</div><p class="muted">Door controller · {firmware} · {count} locks. Open gives one short pulse; the board keeps the lock's rest.</p>
<table class="list"><thead><tr><th>Lock</th><th>Now</th><th></th></tr></thead><tbody>{rows}</tbody></table>{last}{update}</section>"#,
        name = escape(&board.name),
        firmware = escape(firmware),
        count = board
            .details
            .as_ref()
            .and_then(|d| d.lock_count)
            .unwrap_or(locks.len() as u32),
    )
}

#[derive(serde::Deserialize)]
pub(crate) struct OpenForm {
    machine: String,
    lock: u32,
}

/// Open pressed (a Space admin: the route's rule): one `OPEN_LOCK` (part 7), valid for
/// [`OPEN_EXPIRES_S`], only while the board's socket is up.
pub(crate) async fn open_lock(
    access: crate::access::Access,
    axum::extract::State(store): axum::extract::State<crate::Store>,
    axum::Form(form): axum::Form<OpenForm>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let boards = match store.door_controllers(access.space).await {
        Ok(boards) => boards,
        Err(error) => return crate::failed(error),
    };
    let Some(board) = boards.iter().find(|b| b.id == form.machine) else {
        return (StatusCode::NOT_FOUND, "no such door controller").into_response();
    };
    let count = board
        .details
        .as_ref()
        .and_then(|d| d.lock_count)
        .unwrap_or(0);
    if form.lock >= count {
        return (StatusCode::BAD_REQUEST, "no such lock").into_response();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let command = serde_json::json!({
        "command": "OPEN_LOCK",
        "kwargs": {"lock": form.lock, "expires_at": now + OPEN_EXPIRES_S},
    });
    let label = format!("Open lock {}", form.lock + 1);
    let mut legacy = store.lock().unwrap();
    if !legacy.linked(&board.id) {
        return (StatusCode::CONFLICT, "the board is not connected").into_response();
    }
    match legacy.queue_connect_command(Some(&board.link), &board.id, &label, &command, false) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => (StatusCode::NOT_FOUND, "this board takes no commands").into_response(),
    }
}

const UPDATE_LABEL: &str = "Update firmware";

/// Update pressed (a Space admin): part 6b's `UPGRADE` without kwargs; the board takes this hub's
/// build ([`firmware_build`]). Only while its socket is up and it is idle.
pub(crate) async fn update(
    access: crate::access::Access,
    axum::extract::State(store): axum::extract::State<crate::Store>,
    axum::Form(form): axum::Form<UpdateForm>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let boards = match store.door_controllers(access.space).await {
        Ok(boards) => boards,
        Err(error) => return crate::failed(error),
    };
    let Some(board) = boards.iter().find(|b| b.id == form.machine) else {
        return (StatusCode::NOT_FOUND, "no such door controller").into_response();
    };
    if firmware_build(&store).is_none() {
        return (
            StatusCode::NOT_FOUND,
            "no build for door controllers on this hub",
        )
            .into_response();
    }
    let idle = board
        .telemetry
        .as_ref()
        .is_some_and(|t| t.state == maker_connect_protocol::MachineState::Idle);
    let mut legacy = store.lock().unwrap();
    if !legacy.linked(&board.id) || !idle {
        return (StatusCode::CONFLICT, "the board is not connected and idle").into_response();
    }
    let command = serde_json::json!({ "command": "UPGRADE" });
    match legacy.queue_connect_command(Some(&board.link), &board.id, UPDATE_LABEL, &command, false)
    {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => (StatusCode::NOT_FOUND, "this board takes no commands").into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct UpdateForm {
    machine: String,
}

#[cfg(test)]
mod tests {
    use crate::link::tests::call;
    use crate::store::{CommandState, Details, LINK_GONE};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use maker_connect_protocol::MachineKind;

    fn open(machine: &str, lock: u32) -> Request<Body> {
        Request::post("/space/access/open")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("machine={machine}&lock={lock}")))
            .unwrap()
    }

    /// Open queues one `OPEN_LOCK` for 5 s (part 7) only while the board's socket is up, refuses a
    /// lock the board does not have, and a waiting one leaves the queue when the socket goes.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn open_sends_one_short_lived_open_lock_while_the_board_is_there(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (id, token, code) = store
            .register_linked(
                "Door controller 4B28",
                MachineKind::DoorController,
                "Door controller firmware 0.1.0",
                "prusa-connect",
            )
            .await
            .unwrap();
        let space = store.default_space().await.unwrap().0;
        store.pair(space, &code).await.unwrap().unwrap();
        let details = Details {
            lock_count: Some(2),
            ..Details::default()
        };
        store.set_details(&token, &details).await.unwrap();

        assert_eq!(
            call(&app, open(&id, 0)).await.0,
            StatusCode::CONFLICT,
            "no socket"
        );
        {
            let mut legacy = store.lock().unwrap();
            legacy.attached.insert(id.clone(), 1);
            legacy.last_frame.insert(id.clone(), crate::store::now());
        }
        assert_eq!(
            call(&app, open(&id, 2)).await.0,
            StatusCode::BAD_REQUEST,
            "lock 2 of 0-1"
        );
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(call(&app, open(&id, 1)).await.0, StatusCode::NO_CONTENT);
        let sent = store.lock().unwrap().open_commands(&id);
        assert_eq!(sent.len(), 1);
        let sent: serde_json::Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(sent["command"], "OPEN_LOCK");
        assert_eq!(sent["kwargs"]["lock"], 1);
        let expires = sent["kwargs"]["expires_at"].as_u64().unwrap();
        assert!(
            (before + 5..=before + 6).contains(&expires),
            "{expires} vs {before}"
        );

        let (_, page) = call(
            &app,
            Request::get("/space/access").body(Body::empty()).unwrap(),
        )
        .await;
        assert!(page.contains("Door controller 4B28"), "{page}");
        assert!(page.contains("Open lock 2"));

        // The socket goes: the waiting Open leaves the queue.
        {
            let mut legacy = store.lock().unwrap();
            legacy.attached.remove(&id);
            legacy.drop_waiting_open_locks(&id);
        }
        let boards = store.door_controllers(space).await.unwrap();
        let last = &boards[0].commands[0];
        assert_eq!(
            (last.state, last.reason.as_deref()),
            (CommandState::Rejected, Some(LINK_GONE))
        );
        assert!(store.lock().unwrap().open_commands(&id).is_empty());
    }
}
