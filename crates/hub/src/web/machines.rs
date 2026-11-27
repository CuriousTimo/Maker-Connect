use crate::store::MoveRefused;
use crate::{
    PAGE, STYLE, Store, access, done, escape, failed, header, label_page, machine_page, profiles,
};
use axum::{
    Form, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        Html, IntoResponse, Redirect, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use maker_connect_protocol as protocol;

pub(crate) async fn machines(
    access: crate::access::Access,
    State(store): State<Store>,
) -> Response {
    match store.machines(access.space).await {
        Ok(machines) => Json(machines).into_response(),
        Err(error) => failed(error),
    }
}

/// A space's machines as JSON.
async fn machines_json(store: &Store, access: &access::Access) -> String {
    store
        .machines(access.space)
        .await
        .map(|machines| serde_json::to_string(&machines).unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_owned())
}

/// The machines now, and again after every change, merged over a quarter second; and every
/// few seconds anyway, as "seen" and "online" change with time alone.
pub(crate) async fn machine_stream(
    access: access::Access,
    State(store): State<Store>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use std::time::Duration;
    use tokio::sync::broadcast::error::TryRecvError;
    let changes = store.lock().unwrap().subscribe();
    let stream = futures_util::stream::unfold(
        (store, changes, true, access),
        |(store, mut changes, first, access)| async move {
            if !first {
                tokio::select! {
                    _ = changes.recv() => {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        while !matches!(changes.try_recv(), Err(TryRecvError::Empty | TryRecvError::Closed)) {}
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
            let event = Event::default().data(machines_json(&store, &access).await);
            Some((Ok(event), (store, changes, false, access)))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub(crate) async fn page(access: crate::access::Access, State(store): State<Store>) -> Response {
    match store.space_name(access.space).await {
        Ok(name) => Html(
            PAGE.replace("{style}", STYLE)
                .replace("{space}", &escape(&name))
                .replace("{header}", &header(&name, "", "machines")),
        )
        .into_response(),
        Err(error) => failed(error),
    }
}

/// Every machine profile, for the machine picker.
pub(crate) async fn profile_list() -> Response {
    #[derive(serde::Serialize)]
    struct Listed<'a> {
        #[serde(flatten)]
        profile: &'a profiles::Profile,
        image: bool,
    }
    let listed: Vec<Listed> = profiles::all()
        .iter()
        .map(|profile| Listed {
            profile,
            image: profile.image().is_some(),
        })
        .collect();
    Json(listed).into_response()
}

/// A profile's picture.
pub(crate) async fn profile_image(
    axum::extract::Path((brand, file)): axum::extract::Path<(String, String)>,
) -> Response {
    let image = file.strip_suffix(".svg").and_then(|model| {
        // A machine without a profile: the drawing of its kind (`_generic/fdm.svg`).
        if brand == "_generic" {
            let kind = serde_json::from_value(serde_json::Value::String(model.to_owned())).ok()?;
            return profiles::generic_image(kind);
        }
        profiles::find(&format!("{brand}/{model}")).and_then(|p| p.image())
    });
    match image {
        Some(svg) => (
            [
                ("content-type", "image/svg+xml"),
                ("cache-control", "max-age=3600"),
            ],
            svg,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such picture").into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct VirtualForm {
    profile: String,
}

pub(crate) async fn add_virtual(
    access: crate::access::Access,
    State(store): State<Store>,
    Form(form): Form<VirtualForm>,
) -> Response {
    let Some(profile) = profiles::find(&form.profile) else {
        return (StatusCode::BAD_REQUEST, "no such profile").into_response();
    };
    match store.add_virtual(access.space, profile).await {
        Ok(id) => Redirect::to(&format!("/machines/{id}")).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct KlipperForm {
    name: String,
}

/// A printer on Klipper: made here, its Settings then give the lines for its moonraker.conf.
pub(crate) async fn add_klipper(
    access: crate::access::Access,
    State(store): State<Store>,
    headers: HeaderMap,
    Form(form): Form<KlipperForm>,
) -> Response {
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return (StatusCode::BAD_REQUEST, "a name of 1 to 60 characters").into_response();
    }
    let added: Result<_, sqlx::Error> = async {
        let id = store.add_moonraker(access.space, name).await?;
        let token = store.moonraker_token(access.space, &id).await?;
        Ok((id, token))
    }
    .await;
    match added {
        // The wizard shows its lines and waits for it.
        Ok((id, token)) if headers.contains_key("x-requested-with") => {
            Json(serde_json::json!({ "id": id, "token": token })).into_response()
        }
        Ok((id, _)) => Redirect::to(&format!("/machines/{id}#settings")).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct ProfileForm {
    /// A profile id, or empty to recognise the machine from what it says.
    profile: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct MoveForm {
    /// The space it goes to, by its id under the hood.
    space: String,
}

/// The other spaces of theirs an instance admin could move a machine to. (uid, "Organisation ·
/// Space"); none for a token. Moving is an instance admin's, and only theirs (design
/// decisions/20261119-device-move.md; admin-pool allowlist #3).
fn move_targets(access: &access::Access, spaces: &[access::SpaceOf]) -> Vec<(String, String)> {
    if !access.instance_admin || access.token_id.is_some() {
        return Vec::new();
    }
    spaces
        .iter()
        .filter(|s| s.id != access.space)
        .map(|s| (s.uid.clone(), format!("{} · {}", s.organisation, s.name)))
        .collect()
}

/// Moves a machine to another space: an instance admin's (the route's gate), any space of the
/// instance (design decisions/20261119-device-move.md).
pub(crate) async fn move_machine(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<MoveForm>,
) -> Response {
    let to = match store.space_by_uid(form.space.trim()).await {
        Ok(Some(to)) => to,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such space").into_response(),
        Err(error) => return failed(error),
    };
    let name = match store.machine(access.space, &id).await {
        Ok(Some(m)) => m.name,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    let moved = store
        .move_machine(access.space, &id, to)
        .await
        .map(|r| r.map(|()| name));
    match moved {
        Ok(Ok(name)) => {
            let (from_name, to_name) = (
                store.space_name(access.space).await.unwrap_or_default(),
                store.space_name(to).await.unwrap_or_default(),
            );
            let _ = store
                .audit(
                    &access,
                    "moved to another space",
                    &name,
                    &format!("to {to_name}"),
                )
                .await;
            let there = access::Access {
                space: to,
                ..access.clone()
            };
            let _ = store
                .audit(&there, "moved here", &name, &format!("from {from_name}"))
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(MoveRefused::NoSuchMachine)) => {
            (StatusCode::NOT_FOUND, "no such machine").into_response()
        }
        Ok(Err(MoveRefused::InThatSpace)) => {
            (StatusCode::CONFLICT, "it is in that space already").into_response()
        }
        Ok(Err(MoveRefused::JobOpen(_))) => (
            StatusCode::CONFLICT,
            axum::Json(serde_json::json!({ "code": "device/JOB_OPEN" })),
        )
            .into_response(),
        Err(error) => failed(error),
    }
}

pub(crate) async fn remove(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let removed = store.remove_machine(access.space, &id).await;
    match removed {
        // Its permissions, group places and requests go with it (ON DELETE CASCADE).
        Ok(true) => done(&headers, "/"),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct HistoryQuery {
    /// How far back, in seconds; the last 2 hours (HISTORY_SECONDS) when not said.
    seconds: Option<i64>,
}

/// A machine's history from the database (crate::store::history): `?seconds=` back from now.
pub(crate) async fn history(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Response {
    match store.link_of(access.space, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    }
    let seconds = query.seconds.unwrap_or(crate::store::HISTORY_SECONDS);
    match store.history(access.space, &id, seconds).await {
        Ok(samples) => Json(samples).into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct NameForm {
    name: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct LabelsForm {
    /// Comma-separated: `MK4-BL, Room 2`.
    labels: String,
}

/// A machine's labels, as typed: to find it, filter on and group by.
pub(crate) async fn set_labels(
    access: access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<LabelsForm>,
) -> Response {
    let labels: Vec<String> = form.labels.split(',').map(str::to_owned).collect();
    match store.set_labels(access.space, &id, &labels).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

pub(crate) async fn rename(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<NameForm>,
) -> Response {
    let name = form.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return (StatusCode::BAD_REQUEST, "a name of 1 to 60 characters").into_response();
    }
    match store.rename(access.space, &id, name).await {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct OutOfUseForm {
    /// Why it is out of use; may be empty.
    #[serde(default)]
    reason: String,
    /// `out` takes it out of use, `back` brings it back.
    action: String,
}

/// The owner takes a machine out of use, with why, or brings it back.
pub(crate) async fn out_of_use(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<OutOfUseForm>,
) -> Response {
    let reason = form.reason.trim();
    if reason.chars().count() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            "a reason of at most 200 characters",
        )
            .into_response();
    }
    let reason = match form.action.as_str() {
        "out" => Some(reason),
        "back" => None,
        _ => return (StatusCode::BAD_REQUEST, "out or back").into_response(),
    };
    match store.set_out_of_use(access.space, &id, reason).await {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct LaserForm {
    /// Watts, or empty for the profile's.
    #[serde(default)]
    watts: String,
}

/// A laser's module: its optical power, as the owner says.
pub(crate) async fn laser_power(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<LaserForm>,
) -> Response {
    let watts = match form.watts.trim().replace(',', ".") {
        empty if empty.is_empty() => None,
        w => match w.parse::<f64>() {
            Ok(w) if (0.1..=200.0).contains(&w) => Some(w),
            _ => return (StatusCode::BAD_REQUEST, "watts: 0.1 to 200").into_response(),
        },
    };
    match store.set_laser_w(access.space, &id, watts).await {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct BedForm {
    /// Millimetres; all three empty: not known.
    #[serde(default)]
    x: String,
    #[serde(default)]
    y: String,
    #[serde(default)]
    z: String,
}

/// A Marlin printer's bed, as its space says it (a profile from its firmware: M115 does not say it).
pub(crate) async fn bed_size(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<BedForm>,
) -> Response {
    let fields = [&form.x, &form.y, &form.z].map(|f| f.trim().replace(',', "."));
    let bed = if fields.iter().all(String::is_empty) {
        None
    } else {
        let mut mm = [0.0; 3];
        for (i, field) in fields.iter().enumerate() {
            match field.parse::<f64>() {
                Ok(v) if (1.0..=2000.0).contains(&v) => mm[i] = v,
                _ => {
                    return (StatusCode::BAD_REQUEST, "bed size: 1 to 2000 mm each")
                        .into_response();
                }
            }
        }
        Some(mm)
    };
    match store.set_bed_mm(access.space, &id, bed).await {
        Ok(true) => done(&headers, &format!("/machines/{id}#settings")),
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

/// Someone took the finished print off the bed: the machine can take the next job.
pub(crate) async fn taken(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let space = access.space;
    match store.print_taken(space, &id).await {
        Ok(true) => done(&headers, &format!("/machines/{id}")),
        Ok(false) => (StatusCode::NOT_FOUND, "no print on its bed").into_response(),
        Err(error) => failed(error),
    }
}

/// Not ready for the next job after all (PrusaLink's `cancel_ready`).
pub(crate) async fn not_ready(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let space = access.space;
    match store.not_ready(space, &id).await {
        Ok(true) => done(&headers, &format!("/machines/{id}")),
        Ok(false) => (StatusCode::CONFLICT, "it is not ready").into_response(),
        Err(error) => failed(error),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct AnswerForm {
    /// The button pressed, by its place among the question's buttons (from 0).
    button: usize,
}

/// A button of the question on a printer's screen, pressed from the website: a Prusa on
/// Connect gets `DIALOG_ACTION` (its dialog and the button's name).
pub(crate) async fn answer(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<AnswerForm>,
) -> Response {
    let space = access.space;
    let machine = match store.machine(space, &id).await {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    let mut store = store.lock().unwrap();
    if let Some((buttons, dialog)) = store.dialogs.get(&id).cloned() {
        // Buddy refuses a DIALOG_ACTION without `dialog_id` (`command.cpp:86`, `:420`).
        let Some(dialog) = dialog else {
            return (StatusCode::CONFLICT, "the printer named no dialog").into_response();
        };
        let Some(name) = buttons.get(form.button) else {
            return (StatusCode::BAD_REQUEST, "no such button").into_response();
        };
        let kwargs = serde_json::json!({ "dialog_id": dialog, "button": name });
        let command = serde_json::json!({ "command": "DIALOG_ACTION", "kwargs": kwargs });
        return match store.queue_connect_command(
            Some(machine.link.as_str()),
            &id,
            &format!("Answer: {name}"),
            &command,
            false,
        ) {
            Some(_) => StatusCode::NO_CONTENT.into_response(),
            None => (StatusCode::CONFLICT, "the printer takes no commands").into_response(),
        };
    }
    (StatusCode::CONFLICT, "it asks nothing now").into_response()
}

pub(crate) async fn choose_profile(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<ProfileForm>,
) -> Response {
    let profile = Some(form.profile.as_str()).filter(|p| !p.is_empty());
    if profile.is_some_and(|p| profiles::find(p).is_none()) {
        return (StatusCode::BAD_REQUEST, "no such profile").into_response();
    }
    match store.set_profile(access.space, &id, profile).await {
        Ok(true) => {
            if let Some(profile) = profile {
                learn_name(&store, access.space, &id, profile).await;
            }
            done(&headers, &format!("/machines/{id}#settings"))
        }
        Ok(false) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

/// A profile chosen for a Marlin printer whose M115 name (`MACHINE_TYPE`) no profile knows: kept,
/// with the firmware it said, so the name can join that profile (#148 step 2; wiki Machine Profiles,
/// "Learned from real printers").
async fn learn_name(store: &Store, space: i64, id: &str, profile: &str) {
    let Ok(Some(machine)) = store.machine(space, id).await else {
        return;
    };
    if !crate::prusa_connect::marlin_agent(&machine) {
        return;
    }
    let Some(info) = machine
        .details
        .as_ref()
        .and_then(|d| d.firmware_info.as_ref())
    else {
        return;
    };
    let field = |key: &str| {
        info.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let Some(name) = field("MACHINE_TYPE") else {
        return;
    };
    let known = profiles::detect(&maker_connect_protocol::Identity {
        machine_type: Some(name.to_owned()),
        ..maker_connect_protocol::Identity::default()
    });
    if known.is_some() {
        return;
    }
    if let Err(error) = store
        .learn_name(space, name, profile, field("FIRMWARE_NAME"))
        .await
    {
        eprintln!("the name {name:?} for {profile} not kept: {error}");
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct MediaForm {
    /// A media id of the machine's profile, or empty: not said.
    media: String,
}

/// The owner says which tape (or roll) is loaded.
pub(crate) async fn choose_media(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<MediaForm>,
) -> Response {
    let space = access.space;
    let machine = match store.machine(space, &id).await {
        Ok(Some(machine)) => machine,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => return failed(error),
    };
    let media = Some(form.media.as_str()).filter(|m| !m.is_empty());
    let profile = machine.profile.as_deref().and_then(profiles::find);
    if let Some(media) = media
        && profile.and_then(|p| profiles::media(p, media)).is_none()
    {
        return (StatusCode::BAD_REQUEST, "not a tape of this printer").into_response();
    }
    match store.set_media(space, &id, media).await {
        // Saved in the background: the page gets the tape back, to draw the label for it.
        Ok(_) if headers.contains_key("x-requested-with") => {
            let machine = match store.machine(space, &id).await {
                Ok(Some(machine)) => machine,
                _ => return StatusCode::NO_CONTENT.into_response(),
            };
            // HS2: a LabelWriter's page gets its roll as its own page draws it.
            let json = if profile.is_some_and(|p| p.firmware.as_deref() == Some("dymo-lw")) {
                crate::pages::labelwriter_page::roll_json(profile, &machine)
            } else {
                label_page::tape_json(profile, &machine)
            };
            ([("content-type", "application/json")], json).into_response()
        }
        Ok(_) => Redirect::to(&format!("/machines/{id}#label")).into_response(),
        Err(error) => failed(error),
    }
}

/// A machine: its profile to choose (by brand), what the profile knows, and its bed.
pub(crate) async fn machine_page(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    let spaces = if access.token_id.is_none() {
        store.spaces_of(access.user_id).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let ppe_checks = store.ppe_checks(access.space).await.unwrap_or_default();
    let grants = match store.grants_of(&access).await {
        Ok(grants) => grants,
        Err(error) => return failed(error),
    };
    let asked = store
        .asked(access.space, access.user_id, &id, access::Level::Use)
        .await
        .ok()
        .flatten();
    let asked_operate = store
        .asked(access.space, access.user_id, &id, access::Level::Operate)
        .await
        .ok()
        .flatten();
    let found = store
        .machine(access.space, &id)
        .await
        .map(|machine| (space, machine));
    let jobs = store
        .jobs_of(access.space, &id, 10)
        .await
        .unwrap_or_default();
    let stats = store.job_stats(access.space, &id).await.unwrap_or_default();
    // The machine's Camera tab, read before the lock.
    let (camera_tab, camera_now) = match &found {
        Ok((_, Some(machine))) => camera_tab(&store, &access, &machine.id, &grants)
            .await
            .unzip(),
        _ => (None, None),
    };
    let store = store.lock().unwrap();
    match found {
        Ok((space, Some(machine))) if machine.kind == protocol::MachineKind::Label => {
            let move_to = move_targets(&access, &spaces);
            // HS2: a LabelWriter has its own page; a LabelManager the label page.
            let profile = machine.profile.as_deref().and_then(profiles::find);
            if profile.is_some_and(|p| p.firmware.as_deref() == Some("dymo-lw")) {
                return Html(crate::pages::labelwriter_page::render(
                    &space, &machine, &move_to,
                ))
                .into_response();
            }
            Html(label_page::render(&space, &machine, &move_to)).into_response()
        }
        Ok((space, Some(machine))) => {
            let extra = machine_page::Extra {
                camera_tab,
                camera_now,
                me: access.user_id,
                staff: access.role >= access::Role::Admin,
                asked,
                asked_operate,
                ppe_until: store.ppe_until(access.user_id, &machine.id),
                ppe_checks,
                may_use: store
                    .may(&access, &machine.id, access::Level::Use, &grants)
                    .unwrap_or(false),
                may_operate: store
                    .may(&access, &machine.id, access::Level::Operate, &grants)
                    .unwrap_or(false),
                move_to: move_targets(&access, &spaces),
                manage: store
                    .may(&access, &machine.id, access::Level::Manage, &grants)
                    .unwrap_or(false),
                jobs,
                stats,
                laser_framed: machine.laser_framed,
                laser_origin_set: machine.laser_origin_set,
            };
            Html(machine_page::render(&space, &machine, &extra)).into_response()
        }
        Ok((_, None)) => (StatusCode::NOT_FOUND, "no such machine").into_response(),
        Err(error) => failed(error),
    }
}

/// A machine's Camera tab, when the hub has cameras and one on it may be watched.
async fn camera_tab(
    store: &Store,
    access: &crate::access::Access,
    machine: &str,
    grants: &access::Grants,
) -> Option<(String, String)> {
    #[cfg(feature = "cameras")]
    return crate::cameras::page::machine_tab(store, access, machine, grants).await;
    #[cfg(not(feature = "cameras"))]
    {
        let _ = (store, access, machine, grants);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::tests::call;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use maker_connect_protocol::{MachineKind, MachineState, Telemetry};

    use tower::ServiceExt;

    /// The page's stream sends the list at once, and again when a machine changes.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn the_list_streams_its_changes(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let response = app
            .oneshot(
                Request::get("/api/machines/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let mut body = response.into_body();
        let mut next = async || {
            let frame = body.frame().await.unwrap().unwrap();
            String::from_utf8_lossy(frame.data_ref().unwrap()).into_owned()
        };
        assert_eq!(next().await, "data: []\n\n");
        let (_, _, code) = store
            .register_linked("Gonzo", MachineKind::Fdm, "0.1.0", "prusa-connect")
            .await
            .unwrap();
        store.pair(1, &code).await.unwrap();
        let changed = tokio::time::timeout(std::time::Duration::from_secs(2), next())
            .await
            .expect("an event within the merge time, not the 5 s tick");
        assert!(changed.contains(r#""name":"Gonzo""#), "{changed}");
    }

    /// A machine that says it is a MINI gets the MINI's profile; the owner can choose
    /// another (a Zaribo, which says it is an MK3S), and its page draws that bed.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_machine_gets_its_profile(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (id, token, code) = store
            .register_linked("Mini", MachineKind::Fdm, "0.1.0", "prusa-connect")
            .await
            .unwrap();
        let pair = Request::post("/pair")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("code={code}")))
            .unwrap();
        call(&app, pair).await;
        let report = Telemetry {
            state: MachineState::Idle,
            identity: Some(maker_connect_protocol::Identity {
                machine_type: Some("Prusa-MINI".into()),
                model_code: Some(120),
                ..Default::default()
            }),
            ..Default::default()
        };
        store.report(&token, &report).await.unwrap();
        let (_, body) = call(
            &app,
            Request::get("/api/machines").body(Body::empty()).unwrap(),
        )
        .await;
        let machines: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(machines[0]["profile"], "prusa/mini");
        assert_eq!(machines[0]["profile_chosen"], false);

        let choose = Request::post(format!("/machines/{id}/profile"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("profile=zaribo%2F320"))
            .unwrap();
        let (status, _) = call(&app, choose).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        // Its next report does not undo the choice.
        store.report(&token, &report).await.unwrap();
        let (status, page) = call(
            &app,
            Request::get(format!("/machines/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains("Zaribo 320 (MK3S)"), "the chosen profile");
        assert!(
            page.contains(r#"width="250" height="210""#),
            "its bed, to scale"
        );
        assert!(page.contains("250 × 210 × 320 mm"));
    }

    /// #148 step 2: a profile chosen for a Marlin printer whose M115 name no profile knows keeps
    /// the name, the profile and its firmware; one chosen for a name a profile knows keeps nothing.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_name_no_profile_knows_is_kept_with_the_profile_chosen(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (space, id, token) = store.add_test_machine("prusa/mini", None).await;
        let says = async |name: &str| {
            let details = crate::store::Details {
                printer_type: Some(crate::prusa_connect::MARLIN_AGENT.to_owned()),
                firmware_info: Some(
                    serde_json::json!({"FIRMWARE_NAME": "Marlin 2.0.8.2", "MACHINE_TYPE": name})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                ..Default::default()
            };
            assert!(store.set_details(&token, &details).await.unwrap());
        };
        let choose = async |profile: &str| {
            let (status, _) = call(
                &app,
                Request::post(format!("/machines/{id}/profile"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "profile={}",
                        profile.replace('/', "%2F")
                    )))
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::SEE_OTHER);
        };
        // A name a profile knows: nothing kept.
        says("Ender-5 Pro").await;
        choose("creality/ender-5-pro").await;
        assert!(store.learned_names(space).await.unwrap().is_empty());
        // A name no profile knows (as OctoPrint's samples show an Ender 5 Pro saying it).
        says("Ender 5 Pro").await;
        choose("creality/ender-5-pro").await;
        let kept = store.learned_names(space).await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            (kept[0].0.as_str(), kept[0].1.as_str(), kept[0].2.as_deref()),
            (
                "Ender 5 Pro",
                "creality/ender-5-pro",
                Some("Marlin 2.0.8.2")
            )
        );
        // Chosen again: still one.
        choose("creality/ender-5-pro").await;
        assert_eq!(store.learned_names(space).await.unwrap().len(), 1);
    }

    /// HS2: a LabelWriter has its own page, a LabelManager the label page; choosing a roll gives
    /// the LabelWriter's page its raster from the port.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_labelwriter_has_its_own_page(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let (writer, manager) = {
            let space = store.default_space().await.unwrap().0;
            let writer = store
                .add_virtual(space, profiles::find("dymo/labelwriter-450").unwrap())
                .await
                .unwrap();
            let manager = store
                .add_virtual(space, profiles::find("dymo/labelmanager-pnp").unwrap())
                .await
                .unwrap();
            (writer, manager)
        };
        crate::auth::tester_may_operate(&store, &writer).await;
        let get = |id: &str| {
            Request::get(format!("/machines/{id}"))
                .body(Body::empty())
                .unwrap()
        };
        let (status, page) = call(&app, get(&writer)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains("data-roll-form") && page.contains("Roll loaded"),
            "the LabelWriter page"
        );
        assert!(
            page.contains(r#"value="S0722530""#),
            "11353 (S0722530) among the rolls"
        );
        assert!(!page.contains("data-tape-form"));
        let (status, page) = call(&app, get(&manager)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains("data-tape-form") && !page.contains("data-roll-form"),
            "the PnP's label page"
        );

        let choose = Request::post(format!("/machines/{writer}/media"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-requested-with", "fetch")
            .body(Body::from("media=S0722530"))
            .unwrap();
        let (status, body) = call(&app, choose).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let roll: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(roll["dots"], serde_json::json!([271, 254]));
        assert_eq!(roll["length_mm"], 25);
        assert_eq!(roll["page_size"], "w72h72.2");
        assert_eq!(
            roll["cells"].as_array().unwrap().len(),
            2,
            "two labels on the page (DYMO Connect)"
        );
        assert_eq!(roll["orientation"], "Landscape", "where the turn starts");
        // A label printer without a profile: only the notice, on both tabs.
        let bare = {
            let space = store.default_space().await.unwrap().0;
            let id = store
                .add_virtual(space, profiles::find("dymo/labelwriter-450").unwrap())
                .await
                .unwrap();
            store.set_profile(space, &id, None).await.unwrap();
            id
        };
        let (_, page) = call(&app, get(&bare)).await;
        assert!(
            page.contains("No profile yet") && page.contains(r#"<div class="grid-label" hidden>"#),
            "{}",
            &page[..200]
        );
        // For looking at the page by hand: `MC_DUMP_PAGES=<dir> cargo test a_labelwriter_has_its_own_page`.
        if let Ok(dir) = std::env::var("MC_DUMP_PAGES") {
            let (_, page) = call(&app, get(&writer)).await;
            std::fs::write(format!("{dir}/lw450.html"), &page).unwrap();
            std::fs::write(format!("{dir}/bare.html"), call(&app, get(&bare)).await.1).unwrap();
        }
    }
}
