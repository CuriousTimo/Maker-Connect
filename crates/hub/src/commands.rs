use crate::{Store, action, failed, jog_command, jog_move, probe_gcode, profiles, store};
use axum::{
    Form,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use maker_connect_protocol as protocol;

/// Why a command may not go now, by the machine's state (PrusaLink's rules for an MK3,
/// `web/controls.py`): paused, no homing, Z or extruding; extruding needs 170 °C and no
/// print running.
fn refused_now(
    action: &str,
    gcode: &str,
    machine: &store::Machine,
    mk3: bool,
) -> Option<&'static str> {
    use protocol::MachineState::{Idle, Paused};
    let t = machine.telemetry.as_ref()?;
    let paused = t.state == protocol::MachineState::Paused;
    let z_move = gcode
        .lines()
        .any(|l| l.starts_with("G1") && l.split_whitespace().any(|w| w.starts_with('Z')));
    let xy_move = gcode.lines().any(|l| {
        l.starts_with("G1")
            && l.split_whitespace()
                .any(|w| w.starts_with('X') || w.starts_with('Y'))
    });
    let extrude = matches!(action, "extrude" | "retract");
    // PrusaLink's `api_printhead`: homing only idle or ready; Z only idle (ready, finished,
    // stopped); X and Y also paused.
    if mk3 && action == "home" && t.state != Idle {
        return Some("homes only when idle");
    }
    if mk3 && z_move && t.state != Idle {
        return Some("moves Z only when idle");
    }
    if mk3 && xy_move && !extrude && !matches!(t.state, Idle | Paused) {
        return Some("moves X and Y only when idle or paused");
    }
    if paused && action == "home" {
        return Some("not while a print is paused: homing would spoil it");
    }
    if paused && z_move {
        return Some("not while a print is paused: only X and Y move then");
    }
    if extrude {
        if matches!(
            t.state,
            protocol::MachineState::Busy | protocol::MachineState::Paused
        ) {
            return Some("not while it prints");
        }
        if t.nozzle_c.is_none_or(|c| c < 170.0) {
            return Some("the nozzle must be at least 170 °C to extrude");
        }
    }
    None
}

/// The preheat presets (material, nozzle °C, bed °C): one list for the head's menu and Controls'
/// Preheat buttons. Prusa Connect's `PREHEAT_PRESETS` (packages/svelte/src/domain/preheat.ts
/// @2e8c62b), the firmware's own list.
pub(crate) const PREHEAT_PRESETS: [(&str, f64, f64); 10] = [
    ("PLA", 215.0, 60.0),
    ("PETG", 230.0, 85.0),
    ("ASA", 260.0, 100.0),
    ("ABS", 255.0, 100.0),
    ("PC", 275.0, 100.0),
    ("FLEX", 240.0, 50.0),
    ("HIPS", 220.0, 100.0),
    ("PP", 240.0, 100.0),
    ("PVB", 215.0, 75.0),
    ("PA", 285.0, 100.0),
];

#[derive(serde::Deserialize, Default)]
pub(crate) struct CommandForm {
    /// One of the website's buttons ([`action`]).
    #[serde(default)]
    action: String,
    /// G-code typed in the terminal.
    #[serde(default)]
    gcode: String,
    /// The jog pad's step (mm) and speed (mm/min).
    #[serde(default)]
    step: Option<f64>,
    #[serde(default)]
    feed: Option<f64>,
    /// A temperature typed for the nozzle or the bed (°C).
    #[serde(default)]
    nozzle: Option<f64>,
    #[serde(default)]
    bed: Option<f64>,
    /// The speed and flow factors typed (%).
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    flow: Option<f64>,
}

/// A speed or flow factor someone typed, as G-code: whole percents in a range a print
/// survives (speed 10 to 300 %, flow 50 to 150 %).
fn factor_command(
    action: &str,
    speed: Option<f64>,
    flow: Option<f64>,
) -> Option<Result<(String, String), String>> {
    let (what, code, value, low, high) = match action {
        "set-speed" => ("Speed", "M220", speed, 10.0, 300.0),
        "set-flow" => ("Flow", "M221", flow, 50.0, 150.0),
        _ => return None,
    };
    let Some(value) = value.filter(|v| v.is_finite()).map(f64::round) else {
        return Some(Err(format!("{what}: type a percentage")));
    };
    if !(low..=high).contains(&value) {
        return Some(Err(format!("{what}: {low} to {high} %")));
    }
    Some(Ok((
        format!("{what} {value} %"),
        format!("{code} S{value}"),
    )))
}

/// The SDK's SET_VALUE for a speed, flow or target that `factor_command` or `heat_command`
/// made: the key PrusaLink's `SetValue` takes and the whole number of its `S`.
fn set_value(action: &str, gcode: &str) -> Option<serde_json::Value> {
    let key = match action {
        "set-speed" => "speed",
        "set-flow" => "flow",
        "set-nozzle" => "nozzle_temperature",
        "set-bed" => "bed_temperature",
        _ => return None,
    };
    let value: i64 = gcode.split_once(" S")?.1.parse().ok()?;
    Some(serde_json::json!({"command": "SET_VALUE", "kwargs": {key: value}}))
}

/// A temperature someone typed, as G-code: whole degrees from 0 (off) to the profile's
/// maximum (or a safe one without a profile).
fn heat_command(
    action: &str,
    nozzle: Option<f64>,
    bed: Option<f64>,
    profile: Option<&profiles::Profile>,
) -> Option<Result<(String, String), String>> {
    let (what, code, value, max) = match action {
        "set-nozzle" => (
            "Nozzle",
            "M104",
            nozzle,
            profile.and_then(|p| p.max_nozzle_c).unwrap_or(260.0),
        ),
        "set-bed" => (
            "Bed",
            "M140",
            bed,
            profile.and_then(|p| p.max_bed_c).unwrap_or(100.0),
        ),
        _ => return None,
    };
    let Some(value) = value.filter(|v| v.is_finite()) else {
        return Some(Err(format!("{what}: type a temperature")));
    };
    let value = value.round();
    if !(0.0..=max).contains(&value) {
        return Some(Err(format!("{what}: 0 to {max} °C on this machine")));
    }
    let label = if value == 0.0 {
        format!("{what} off")
    } else {
        format!("{what} to {value} °C")
    };
    Some(Ok((label, format!("{code} S{value}"))))
}

/// G-code from the terminal: checked, and a label (its first non-blank line, trimmed). The
/// G-code itself is returned unchanged (no trim, no dropping of blank lines): the port splits it
/// as OctoPrint's terminal does. Short enough for every machine: a Prusa on built-in Connect
/// takes 512 bytes.
fn typed_gcode(text: &str) -> Result<(String, String), &'static str> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return Err("no G-code");
    }
    if lines.len() > 20 || text.len() > 480 {
        return Err("too long: at most 20 lines, 480 characters");
    }
    if lines
        .iter()
        .any(|l| l.chars().any(|c| c.is_control() || !c.is_ascii()))
    {
        return Err("G-code is plain text: letters, numbers, spaces");
    }
    let label = match lines.len() {
        1 => lines[0].to_owned(),
        n => format!("{} (+{} lines)", lines[0], n - 1),
    };
    Ok((label, text.to_owned()))
}

/// Why typed G-code to a laser is refused (our text).
pub(crate) const LASER_NO_GCODE: &str =
    "a laser takes no typed G-code: only Start burn switches it on";

pub(crate) async fn command(
    access: crate::access::Access,
    State(store): State<Store>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<CommandForm>,
) -> Response {
    let ppe_checks = store.ppe_rule(access.space, &id).await;
    // The machine as it is now, read once (the website's commands go into memory below).
    let found_machine = store
        .machine(access.space, &id)
        .await
        .map_err(|e| e.to_string());
    let machine_now = || found_machine.clone();
    // How it is reached, for queueing (None: not in the space).
    let link = found_machine
        .as_ref()
        .ok()
        .and_then(|m| m.as_ref())
        .map(|m| m.link.clone());
    let link = link.as_deref();
    // A laser takes no typed G-code: a line could switch it on without a frame, PPE or a person's
    // Start (docs/design/decisions/20261119-no-raw-gcode-on-a-laser.md).
    if !form.gcode.trim().is_empty()
        && let Ok(Some(machine)) = &found_machine
        && machine.kind == protocol::MachineKind::Laser
    {
        return (StatusCode::BAD_REQUEST, LASER_NO_GCODE).into_response();
    }
    let mut store = store.lock().unwrap();
    // Switching things off never waits for PPE; everything else does.
    let off = form.gcode.trim().is_empty()
        && matches!(form.action.as_str(), "cooldown" | "fan-off" | "motors-off");
    if !off && let Err(refused) = crate::safety::check_ppe(&store, &ppe_checks, &access, &id) {
        return *refused;
    }
    if !form.gcode.trim().is_empty() {
        let (label, gcode) = match typed_gcode(&form.gcode) {
            Ok(typed) => typed,
            Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
        };
        return match store.queue_command(link, &id, &label, &gcode) {
            Some(_) => StatusCode::NO_CONTENT.into_response(),
            None => (
                StatusCode::NOT_FOUND,
                "no such machine, or it takes no commands",
            )
                .into_response(),
        };
    }
    let step = form.step.unwrap_or(10.0);
    let feed = form.feed.unwrap_or(3000.0);
    // A laser on the agent: Part 4's commands, typed (docs/design/prusa-sdk-extension.md).
    if let Ok(Some(machine)) = machine_now()
        && crate::prusa_connect::laser_agent(&machine)
    {
        let Some((label, command)) = laser_command(&form.action, step, feed) else {
            return (StatusCode::BAD_REQUEST, "no such command").into_response();
        };
        let queued = store.queue_connect_command(link, &id, &label, &command, false);
        return queued_response(queued, &headers, &id);
    }
    let owned = |(l, g): (&str, &str)| (l.to_owned(), g.to_owned());
    let found = if form.action.starts_with("jog") {
        jog_command(&form.action, step, feed)
    } else if let Some(found) = factor_command(&form.action, form.speed, form.flow) {
        match found {
            Ok(found) => Some(found),
            Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
        }
    } else if matches!(form.action.as_str(), "set-nozzle" | "set-bed") {
        let machine = match machine_now() {
            Ok(Some(machine)) => machine,
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        };
        let profile = machine.profile.as_deref().and_then(profiles::find);
        match heat_command(&form.action, form.nozzle, form.bed, profile) {
            Some(Ok(found)) => Some(found),
            Some(Err(why)) => return (StatusCode::BAD_REQUEST, why).into_response(),
            None => None,
        }
    } else if matches!(form.action.as_str(), "load-filament" | "unload-filament") {
        // Hot first, to the loaded spool's temperature (215 °C when it is not known), then the
        // firmware's own load or unload (Marlin's and Prusa's M701, M702).
        let machine = match machine_now() {
            Ok(Some(machine)) => machine,
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        };
        if machine.kind != protocol::MachineKind::Fdm {
            return (StatusCode::BAD_REQUEST, "filament goes into a 3D printer").into_response();
        }
        let hot = machine
            .loaded
            .as_ref()
            .and_then(|l| l.nozzle_c)
            .filter(|c| (150.0..=320.0).contains(c))
            .unwrap_or(215.0);
        let load = form.action == "load-filament";
        let label = if load {
            "Load filament"
        } else {
            "Unload filament"
        };
        Some((
            format!("{label} ({hot:.0} °C)"),
            format!("M109 S{hot:.0}\n{}", if load { "M701" } else { "M702" }),
        ))
    } else if form.action == "probe-bed" {
        match machine_now() {
            Ok(Some(machine)) => match probe_gcode(&machine) {
                Ok(found) => Some(owned(found)),
                Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
            },
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        }
    } else {
        action(&form.action).map(owned)
    };
    let Some((label, gcode)) = found else {
        return (StatusCode::BAD_REQUEST, "no such command").into_response();
    };
    // As PrusaLink allows them (web/controls.py, api_printhead and api_tool): while paused
    // (a print it runs itself, from its card or its menu) no homing, no Z move and no
    // extruding, which would spoil the print; X and Y may move. Extruding wants a hot
    // nozzle (170 °C, PrusaLink's `min_temp_nozzle_e`).
    if let Ok(Some(machine)) = machine_now()
        && let Some(why) = refused_now(&form.action, &gcode, &machine, false)
    {
        return (StatusCode::CONFLICT, why).into_response();
    }
    // A Marlin printer on the agent takes no G-code while it prints (PrusaLink's rule,
    // `prusa_link.py:545-552`): speed, flow and the targets go as the SDK's SET_VALUE
    // (`command_handlers.py:395-415`; docs/design/prusa-sdk-extension.md part 9).
    let set_value = match machine_now() {
        Ok(Some(machine)) if crate::prusa_connect::marlin_agent(&machine) => {
            set_value(&form.action, &gcode)
        }
        _ => None,
    };
    let queued = match set_value {
        Some(command) => store.queue_connect_command(link, &id, &label, &command, false),
        None => store.queue_command(link, &id, &label, &gcode),
    };
    queued_response(queued, &headers, &id)
}

/// The answer to a command queued (or not) from the website.
fn queued_response(queued: Option<u32>, headers: &HeaderMap, id: &str) -> Response {
    match queued {
        // The page sends it without leaving; a plain form comes back to the page.
        Some(_) if headers.contains_key("x-requested-with") => {
            StatusCode::NO_CONTENT.into_response()
        }
        Some(_) => Redirect::to(&format!("/machines/{id}#control")).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "no such machine, or it takes no commands",
        )
            .into_response(),
    }
}

/// Part 4's commands for a laser on the agent: a button as the SDK command, and its label. A jog
/// by the pad's steps and speeds only (`jog_move`: the hub's UI limits), X and Y only.
fn laser_command(action: &str, step: f64, feed: f64) -> Option<(String, serde_json::Value)> {
    let (label, name) = match action {
        "home" => ("Home", "HOME"),
        "unlock" => ("Unlock", "UNLOCK"),
        "set-origin" => ("Set origin here", "SET_ORIGIN"),
        "go-origin" => ("Go to origin", "GO_TO_ORIGIN"),
        _ => {
            let (label, [x, y, z], feed) = jog_move(action, step, feed)?;
            if z != 0.0 {
                return None;
            }
            let kwargs = serde_json::json!({"x": x, "y": y, "feed": feed});
            return Some((
                label,
                serde_json::json!({"command": "JOG", "kwargs": kwargs}),
            ));
        }
    };
    Some((label.to_owned(), serde_json::json!({"command": name})))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::link::tests::call;

    /// A laser on the agent: its buttons as Part 4's commands; the pad's steps and speeds only,
    /// no Z, nothing else.
    #[test]
    fn a_laser_on_the_agent_gets_part_4s_commands() {
        let json = |a: &str| laser_command(a, 10.0, 3000.0).map(|(_, c)| c);
        assert_eq!(json("home"), Some(serde_json::json!({"command": "HOME"})));
        assert_eq!(
            json("unlock"),
            Some(serde_json::json!({"command": "UNLOCK"}))
        );
        assert_eq!(
            json("set-origin"),
            Some(serde_json::json!({"command": "SET_ORIGIN"}))
        );
        assert_eq!(
            json("go-origin"),
            Some(serde_json::json!({"command": "GO_TO_ORIGIN"}))
        );
        assert_eq!(
            laser_command("jog:1:-1", 0.1, 600.0),
            Some((
                "Move X+0.1 Y−0.1 mm".to_owned(),
                serde_json::json!({"command": "JOG", "kwargs": {"x": 0.1, "y": -0.1, "feed": 600.0}})
            ))
        );
        assert_eq!(json("jog:0:0:1"), None, "no Z");
        assert_eq!(
            laser_command("jog:1:0", 500.0, 3000.0),
            None,
            "not a step offered"
        );
        assert_eq!(json("settings"), None, "no Read settings");
        assert_eq!(json("cooldown"), None);
    }

    /// Controls' Preheat buttons and the legacy G-code follow PREHEAT_PRESETS (one list).
    #[test]
    fn the_preheat_buttons_are_the_presets() {
        for (action, material) in [("preheat-pla", "PLA"), ("preheat-petg", "PETG")] {
            let (_, nozzle, bed) = PREHEAT_PRESETS.iter().find(|p| p.0 == material).unwrap();
            let (label, gcode) = crate::devices::legacy_gcode::action(action).unwrap();
            assert_eq!(gcode, format!("M104 S{nozzle}\nM140 S{bed}"), "{action}");
            assert!(label.contains(&format!("({nozzle}/{bed} °C)")), "{label}");
        }
    }

    use axum::body::Body;
    use axum::http::Request;

    /// Typed temperatures: whole degrees, up to the profile's maximum, 0 is off.
    #[test]
    fn typed_temperatures_stay_within_the_machine() {
        let mk3s = profiles::find("prusa/mk3s");
        assert!(mk3s.is_some());
        assert_eq!(
            heat_command("set-nozzle", Some(212.6), None, mk3s),
            Some(Ok(("Nozzle to 213 °C".to_owned(), "M104 S213".to_owned())))
        );
        assert_eq!(
            heat_command("set-bed", None, Some(0.0), mk3s),
            Some(Ok(("Bed off".to_owned(), "M140 S0".to_owned())))
        );
        assert!(matches!(
            heat_command("set-bed", None, Some(121.0), mk3s),
            Some(Err(_))
        ));
        assert!(matches!(
            heat_command("set-nozzle", Some(-5.0), None, mk3s),
            Some(Err(_))
        ));
        assert!(matches!(
            heat_command("set-nozzle", None, None, mk3s),
            Some(Err(_))
        ));
        assert!(matches!(
            heat_command("set-nozzle", Some(270.0), None, None),
            Some(Err(_))
        ));
        assert_eq!(heat_command("set-origin", None, None, mk3s), None);
        assert_eq!(
            factor_command("set-speed", Some(149.6), None),
            Some(Ok(("Speed 150 %".to_owned(), "M220 S150".to_owned())))
        );
        assert!(matches!(
            factor_command("set-flow", None, Some(30.0)),
            Some(Err(_))
        ));
        assert!(matches!(
            factor_command("set-speed", None, None),
            Some(Err(_))
        ));
        assert_eq!(factor_command("set-bed", None, None), None);
    }

    /// "Set speed" sends M220 S with whole percents from 10 to 300.
    /// Proves the row `M220 S` of docs/audits/prusalink.md.
    #[test]
    fn the_speed_goes_by_m220_s() {
        let sent = |v| factor_command("set-speed", Some(v), None);
        assert_eq!(sent(10.0).unwrap().unwrap().1, "M220 S10");
        assert_eq!(sent(300.0).unwrap().unwrap().1, "M220 S300");
        assert_eq!(sent(120.4).unwrap().unwrap().1, "M220 S120");
        assert!(sent(9.0).unwrap().is_err(), "M220 below 10 %");
        assert!(sent(301.0).unwrap().is_err(), "M220 above 300 %");
    }

    /// "Set flow" sends M221 S with whole percents from 50 to 150.
    /// Proves the row `M221 S` of docs/audits/prusalink.md.
    #[test]
    fn the_flow_goes_by_m221_s() {
        let sent = |v| factor_command("set-flow", None, Some(v));
        assert_eq!(sent(50.0).unwrap().unwrap().1, "M221 S50");
        assert_eq!(sent(150.0).unwrap().unwrap().1, "M221 S150");
        assert!(sent(49.0).unwrap().is_err(), "M221 below 50 %");
        assert!(sent(151.0).unwrap().is_err(), "M221 above 150 %");
    }

    /// Typed G-code is tidied and kept short enough for every machine.
    #[test]
    fn typed_gcode_is_checked() {
        assert_eq!(
            typed_gcode(" G28 \n\nG1 X10\n"),
            Ok(("G28 (+1 lines)".to_owned(), " G28 \n\nG1 X10\n".to_owned()))
        );
        assert!(typed_gcode("   ").is_err());
        assert!(typed_gcode(&"G4 P0\n".repeat(21)).is_err());
        assert!(typed_gcode("M117 \u{7}").is_err());
    }

    /// A machine the signed-in tester may use (not operate): a virtual machine of `profile`, idle.
    async fn usable(store: &crate::Store, profile: &str) -> String {
        let (space, id, _) = store.add_test_machine(profile, None).await;
        let user = store.user_id("tester").await.unwrap().unwrap();
        store
            .set_grant(space, &id, user, crate::access::Level::Use, user, None)
            .await
            .unwrap();
        store.idle_test_machine(&id).await;
        id
    }

    /// The control rule (ui.md): without 'operate' the page shows no button that sends a command
    /// (pad, terminal, probe), one notice instead; with it, the pad is there.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn without_operate_the_controls_are_hidden(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let id = usable(&store, "prusa/mk3s").await;
        let page = || {
            Request::get(format!("/machines/{id}"))
                .body(Body::empty())
                .unwrap()
        };
        let html = call(&app, page()).await.1;
        assert!(
            html.contains("You may not move, heat or send G-code"),
            "{html}"
        );
        for sends in [r#"value="jog:0:1""#, "data-gcode ", r#"value="probe-bed""#] {
            assert!(!html.contains(sends), "{sends} shown without operate");
        }
        crate::auth::tester_may_operate(&store, &id).await;
        let html = call(&app, page()).await.1;
        assert!(!html.contains("You may not move, heat or send G-code"));
        assert!(html.contains(r#"value="jog:0:1""#));
        assert!(html.contains(r#"value="jog:0:0:-1""#));
    }

    /// A laser takes no typed G-code, also for whoever may operate it (decision
    /// 20261119-no-raw-gcode-on-a-laser); its page has no terminal and no Advanced tab, and its
    /// Controls are one card (laser.html): Coordinates with the pad, Origin and Machine.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_laser_takes_no_typed_gcode(pool: sqlx::PgPool) {
        let store = crate::store::Store::for_test(&pool).await;
        let app = crate::auth::signed_in_app(store.clone()).await;
        let laser = usable(&store, "ortur/lm2-pro-s2").await;
        let user = store.user_id("tester").await.unwrap().unwrap();
        let space = store.default_space().await.unwrap().0;
        store
            .set_grant(
                space,
                &laser,
                user,
                crate::access::Level::Operate,
                user,
                None,
            )
            .await
            .unwrap();
        for gcode in ["M3 S1000", "M105"] {
            let typed = Request::post(format!("/machines/{laser}/command"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("gcode={gcode}")))
                .unwrap();
            let (status, why) = call(&app, typed).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{gcode}");
            assert_eq!(why, LASER_NO_GCODE);
        }
        assert!(
            store.lock().unwrap().open_commands(&laser).is_empty(),
            "nothing sent"
        );
        let page = call(
            &app,
            Request::get(format!("/machines/{laser}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .1;
        assert!(!page.contains(r#"<pre class="terminal""#), "no terminal");
        assert!(!page.contains(r##"href="#advanced""##), "no Advanced tab");
        assert!(page.contains("<h4>Coordinates</h4>") && page.contains(r#"class="ctl-cols""#));
        assert!(page.contains(r#"value="set-origin""#) && page.contains(r#"value="jog:1:1""#));
        assert!(page.contains(r#"data-live="pos_x""#), "X in the head");
        // Idle: nothing to stop (control_job answers 404), so no Stop and no hint.
        let actions = page
            .split(r#"<form class="job-actions""#)
            .nth(1)
            .unwrap()
            .split('>')
            .next()
            .unwrap();
        assert!(actions.contains("hidden"), "no Stop for an idle laser");
        let hint = page
            .split("data-laser-stop")
            .nth(1)
            .unwrap()
            .split('>')
            .next()
            .unwrap();
        assert!(hint.contains("hidden"), "no Stop hint for an idle laser");
    }
}
