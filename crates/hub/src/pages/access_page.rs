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
        html.push_str(r#"<section class="card"><h2>Door controllers</h2><p class="muted">None yet. A door controller is added like a machine: its setup page shows a code, typed in under Add machine.</p></section>"#);
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

/// A lock's name: the one given on this page, or its number.
fn lock_name(board: &crate::store::Machine, lock: u32) -> String {
    board
        .lock_names
        .get(lock as usize)
        .filter(|n| !n.is_empty())
        .cloned()
        .unwrap_or_else(|| format!("Lock {}", lock + 1))
}

/// One board (owner, 2026-11-27: mockup variant A): a card with its state, firmware and update,
/// a tile per lock with its door, rest, Open and Rename, and the last Opens.
fn board_card(store: &crate::Store, board: &crate::store::Machine) -> String {
    use crate::escape;
    use maker_connect_protocol::DoorReading;
    let linked = store.lock().unwrap().linked(&board.id);
    let telemetry = board.telemetry.as_ref();
    // The version the board says of itself (its INFO), else what its telemetry carries.
    let firmware = board
        .details
        .as_ref()
        .and_then(|d| d.firmware.as_deref())
        .or_else(|| telemetry.and_then(|t| t.firmware.as_deref()))
        .unwrap_or("–");
    let link = if linked {
        r#"<span class="st wait">Connected</span>"#
    } else {
        r#"<span class="st bad">Not connected</span>"#
    };
    let locks = telemetry.and_then(|t| t.locks.clone()).unwrap_or_default();
    let count = board
        .details
        .as_ref()
        .and_then(|d| d.lock_count)
        .unwrap_or(locks.len() as u32);
    let id = escape(&board.id);
    // The build this hub carries, offered while the board is there and idle (part 6b's guard).
    let build = firmware_build(store)
        .and_then(|p| std::fs::read(p).ok())
        .map(|image| build_version(&image).unwrap_or_else(|| "unknown version".to_owned()));
    let idle = telemetry.is_some_and(|t| t.state == maker_connect_protocol::MachineState::Idle);
    let update = match &build {
        Some(version) if linked && idle => format!(
            r#"<form method="post" action="space/access/update" data-async data-done="Updating: the board restarts" class="upd"><input type="hidden" name="machine" value="{id}"><span class="grow muted">Build on this hub: <b>{v}</b></span><span class="fw"><button class="btn small">Update firmware</button></span></form>"#,
            v = escape(version)
        ),
        Some(version) => format!(
            r#"<div class="upd"><span class="grow muted">Build on this hub: <b>{}</b>. Update when the board is connected and idle.</span></div>"#,
            escape(version)
        ),
        None => String::new(),
    };
    let mut tiles = String::new();
    let mut unused = String::new();
    // Every lock the board has (its INFO), with what its telemetry last said of it.
    for n in 0..count {
        let reading = locks.iter().find(|l| l.lock == n);
        let door = reading.and_then(|l| l.door);
        let resting_s = reading.map_or(0, |l| l.resting_s);
        if board.locks_off.contains(&n) {
            unused.push_str(&format!(
                r#"<li><b>{name}</b><small class="muted">relay {shown}</small><form method="post" action="space/access/in-use" data-async data-done="In use"><input type="hidden" name="machine" value="{id}"><input type="hidden" name="lock" value="{n}"><input type="hidden" name="in_use" value="1"><button class="btn small">Use</button></form></li>"#,
                name = escape(&lock_name(board, n)),
                shown = n + 1
            ));
            continue;
        }
        let shown = n + 1;
        let name = escape(&lock_name(board, n));
        let given = board
            .lock_names
            .get(n as usize)
            .map(|g| escape(g))
            .unwrap_or_default();
        let (class, now) = match door {
            None => ("bad", "Door unknown"),
            Some(DoorReading::Closed) => ("wait", "Door closed"),
            Some(DoorReading::Open) => ("warn", "Door open"),
        };
        let rest = if resting_s > 0 {
            format!(r#"Resting <b data-rest="{0}">{0}</b> s"#, resting_s)
        } else {
            String::new()
        };
        let resting = if resting_s > 0 { " on" } else { "" };
        let button = if linked {
            format!(
                r#"<form method="post" action="space/access/open" data-async data-done="Sent"><input type="hidden" name="machine" value="{id}"><input type="hidden" name="lock" value="{n}"><span class="fw"><button class="btn small primary">Open</button></span></form>"#
            )
        } else {
            r#"<span class="fw"><button class="btn small primary" disabled title="The board is not connected">Open</button></span>"#.to_owned()
        };
        tiles.push_str(&format!(
            r#"<article class="card lk{resting}"><div class="tp"><h3>{name}</h3><small class="muted">relay {shown} · input {shown}</small></div><div><span class="st {class}">{now}</span></div><div class="rest">{rest}</div>{button}<details class="rn"><summary>Settings</summary><form method="post" action="space/access/name" data-async data-done="Saved" class="a-form"><input type="hidden" name="machine" value="{id}"><input type="hidden" name="lock" value="{n}"><input name="name" value="{given}" maxlength="40" placeholder="Lock {shown}" aria-label="Name of lock {shown}"><button class="btn small">Save</button></form><form method="post" action="space/access/in-use" data-async data-done="Not in use"><input type="hidden" name="machine" value="{id}"><input type="hidden" name="lock" value="{n}"><input type="hidden" name="in_use" value="0"><button class="btn small ghost">Not in use: hide this lock</button></form></details></article>"#
        ));
    }
    if count == 0 {
        tiles.push_str(r#"<p class="muted">No locks reported yet.</p>"#);
    }
    let unused = if unused.is_empty() {
        String::new()
    } else {
        format!(
            r#"<section class="card"><h3>Not in use</h3><p class="muted">Relays without a lock: hidden above, never opened.</p><ul class="lastlist">{unused}</ul></section>"#
        )
    };
    // The last Opens and how they ended: "Lock resting", "Expired" as the board says them.
    let last: String = board
        .commands
        .iter()
        .filter(|c| c.label.starts_with("Open lock") || c.label == UPDATE_LABEL)
        .take(5)
        .map(|c| {
            use crate::store::CommandState;
            let (class, how) = match c.state {
                CommandState::Queued => ("", "waiting".to_owned()),
                CommandState::Sent => ("", "sent".to_owned()),
                CommandState::Accepted => ("", "busy".to_owned()),
                CommandState::Finished if c.label == UPDATE_LABEL => ("ok", "updated".to_owned()),
                CommandState::Finished => ("ok", "opened".to_owned()),
                CommandState::Rejected => (
                    "no",
                    format!("not done: {}", c.reason.as_deref().unwrap_or("refused")),
                ),
            };
            // "Open lock 3" with the lock's name as it is now.
            let label = c
                .label
                .strip_prefix("Open lock ")
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n >= 1)
                .map_or_else(
                    || c.label.clone(),
                    |n| format!("Open {}", lock_name(board, n - 1)),
                );
            let by =
                c.by.as_deref()
                    .map(|by| format!(r#"<span class="muted">{}</span>"#, escape(by)))
                    .unwrap_or_default();
            format!(
                r#"<li><b>{}</b>{by}<span class="out {class}">{}</span></li>"#,
                escape(&label),
                escape(&how)
            )
        })
        .collect();
    let last = if last.is_empty() {
        String::new()
    } else {
        format!(
            r#"<section class="card"><h3>Last opens</h3><ul class="lastlist">{last}</ul></section>"#
        )
    };
    format!(
        r#"<div class="ac"><section class="card"><div class="bh"><h2>{name}</h2>{link}</div><ul class="facts"><li><small class="muted">Firmware</small><b>{firmware}</b></li><li><small class="muted">Locks</small><b>{count}</b></li></ul>{update}</section><div class="locks">{tiles}</div>{unused}{last}</div>"#,
        name = escape(&board.name),
        firmware = escape(firmware),
    )
}

#[derive(serde::Deserialize)]
pub(crate) struct InUseForm {
    machine: String,
    lock: u32,
    in_use: u8,
}

/// A relay that drives a lock, or not (6 relays, 4 lockers): one not in use is hidden and never
/// opened.
pub(crate) async fn lock_in_use(
    access: crate::access::Access,
    axum::extract::State(store): axum::extract::State<crate::Store>,
    axum::Form(form): axum::Form<InUseForm>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    if form.lock >= 64 {
        return (StatusCode::BAD_REQUEST, "no such lock").into_response();
    }
    match store
        .set_lock_in_use(access.space, &form.machine, form.lock, form.in_use != 0)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such door controller").into_response(),
        Err(error) => crate::failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct NameForm {
    machine: String,
    lock: u32,
    name: String,
}

/// Rename pressed: the lock's own name on this hub (empty: its number again). The board does not
/// know names; only the hub shows them.
pub(crate) async fn name_lock(
    access: crate::access::Access,
    axum::extract::State(store): axum::extract::State<crate::Store>,
    axum::Form(form): axum::Form<NameForm>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    if form.lock >= 64 {
        return (StatusCode::BAD_REQUEST, "no such lock").into_response();
    }
    match store
        .set_lock_name(access.space, &form.machine, form.lock, &form.name)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such door controller").into_response(),
        Err(error) => crate::failed(error),
    }
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
    if board.locks_off.contains(&form.lock) {
        return (StatusCode::CONFLICT, "this lock is not in use").into_response();
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
        Some(id) => {
            legacy.command_by(&board.id, id, &access.actor);
            StatusCode::NO_CONTENT.into_response()
        }
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
        Some(id) => {
            legacy.command_by(&board.id, id, &access.actor);
            StatusCode::NO_CONTENT.into_response()
        }
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
        assert!(page.contains("Open Lock 2"), "{page}");

        // A name for lock 2: the page and its last Opens use it; an empty name gives the number back.
        let rename = |name: &str| {
            Request::post("/space/access/name")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("machine={id}&lock=1&name={name}")))
                .unwrap()
        };
        assert_eq!(
            call(&app, rename("Front+door")).await.0,
            StatusCode::NO_CONTENT
        );
        let boards = store.door_controllers(space).await.unwrap();
        assert_eq!(boards[0].lock_names, ["", "Front door"]);
        let (_, page) = call(
            &app,
            Request::get("/space/access").body(Body::empty()).unwrap(),
        )
        .await;
        assert!(page.contains("Open Front door"), "{page}");
        // Who pressed Open is kept with it and shown.
        let by = boards[0].commands[0].by.clone().expect("who opened it");
        assert!(
            page.contains(&format!(r#"<span class="muted">{by}</span>"#)),
            "{page}"
        );
        // Lock 2 not in use: hidden from the tiles, and Open is refused; in use again, it is back.
        let in_use = |on: u8| {
            Request::post("/space/access/in-use")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("machine={id}&lock=1&in_use={on}")))
                .unwrap()
        };
        assert_eq!(call(&app, in_use(0)).await.0, StatusCode::NO_CONTENT);
        assert_eq!(
            store.door_controllers(space).await.unwrap()[0].locks_off,
            [1]
        );
        let (_, page) = call(
            &app,
            Request::get("/space/access").body(Body::empty()).unwrap(),
        )
        .await;
        assert!(page.contains("Not in use"), "{page}");
        assert!(!page.contains("<h3>Front door</h3>"), "{page}");
        assert_eq!(call(&app, open(&id, 1)).await.0, StatusCode::CONFLICT);
        assert_eq!(call(&app, in_use(1)).await.0, StatusCode::NO_CONTENT);
        assert!(
            store.door_controllers(space).await.unwrap()[0]
                .locks_off
                .is_empty()
        );
        assert_eq!(call(&app, rename("")).await.0, StatusCode::NO_CONTENT);
        let boards = store.door_controllers(space).await.unwrap();
        assert!(boards[0].lock_names.is_empty());

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
