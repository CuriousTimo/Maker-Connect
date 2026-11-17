use crate::{
    JOG_FEEDS, JOG_STEPS, Shared, action, failed, grbl_action, jog_command, probe_gcode, profiles,
    store,
};
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
fn refused_now(action: &str, gcode: &str, machine: &store::Machine, mk3: bool) -> Option<&'static str> {
    use protocol::MachineState::{Idle, Paused};
    let t = machine.telemetry.as_ref()?;
    let paused = t.state == protocol::MachineState::Paused;
    let z_move = gcode.lines().any(|l| l.starts_with("G1") && l.split_whitespace().any(|w| w.starts_with('Z')));
    let xy_move = gcode.lines().any(|l| l.starts_with("G1") && l.split_whitespace().any(|w| w.starts_with('X') || w.starts_with('Y')));
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
        if matches!(t.state, protocol::MachineState::Busy | protocol::MachineState::Paused) {
            return Some("not while it prints");
        }
        if t.nozzle_c.is_none_or(|c| c < 170.0) {
            return Some("the nozzle must be at least 170 °C to extrude");
        }
    }
    None
}

/// The commands the website offers: a label and its G-code. Moves are relative, 10 mm.
/// A button of the website for a machine behind an agent, as typed commands (exactly the
/// controls the pages offer, owner's approval O8). Its step and speed from the page's lists
/// (`JOG_STEPS`, `JOG_FEEDS`), the preheat temperatures the page's own: limits of the page,
/// not of the wire.
fn agent_actions(form: &CommandForm, machine: &store::Machine) -> Result<Vec<(String, protocol::Action)>, String> {
    use protocol::{Action, Axis, Heater};
    let one = |label: &str, action: Action| Ok(vec![(label.to_owned(), action)]);
    let step = form.step.unwrap_or(10.0);
    if !JOG_STEPS.contains(&step) {
        return Err("Move: no such step".to_owned());
    }
    if form.feed.is_some_and(|f| !JOG_FEEDS.contains(&f)) {
        return Err("Move: no such speed".to_owned());
    }
    let jog = |label: String, x: Option<f64>, y: Option<f64>, z: Option<f64>| {
        Ok(vec![(label, Action::Jog { x, y, z, feed: form.feed })])
    };
    let heat = |label: &str, nozzle: f64, bed: f64| {
        Ok(vec![
            (label.to_owned(), Action::Temperature { heater: Heater::Nozzle, celsius: nozzle }),
            (label.to_owned(), Action::Temperature { heater: Heater::Bed, celsius: bed }),
        ])
    };
    let spool = (
        machine.loaded.as_ref().and_then(|l| l.nozzle_c).map(|c| c as f32),
        machine.loaded.as_ref().and_then(|l| l.bed_c).map(|c| c as f32),
    );
    match form.action.as_str() {
        // B5: OctoPrint's panel has two Home buttons, X/Y and Z (`control.jinja2:71,85`); the one
        // button of the other machines is all three axes (K5).
        "home" => one("Home", Action::Home { axes: vec![Axis::X, Axis::Y, Axis::Z] }),
        "home-xy" => one("Home X/Y", Action::Home { axes: vec![Axis::X, Axis::Y] }),
        "home-z" => one("Home Z", Action::Home { axes: vec![Axis::Z] }),
        "x+" => jog(format!("X +{step} mm"), Some(step), None, None),
        "x-" => jog(format!("X −{step} mm"), Some(-step), None, None),
        "y+" => jog(format!("Y +{step} mm"), None, Some(step), None),
        "y-" => jog(format!("Y −{step} mm"), None, Some(-step), None),
        "z+" => jog(format!("Z +{step} mm"), None, None, Some(step)),
        "z-" => jog(format!("Z −{step} mm"), None, None, Some(-step)),
        jog_pad if jog_pad.starts_with("jog:") => {
            let dir = |t: &str| match t {
                "-1" => Some(-1.0),
                "0" => Some(0.0),
                "1" => Some(1.0),
                _ => None,
            };
            let (x, y) = jog_pad["jog:".len()..].split_once(':').ok_or("Move: no such direction")?;
            let (x, y) = (dir(x).ok_or("Move: no such direction")?, dir(y).ok_or("Move: no such direction")?);
            let some = |d: f64| (d != 0.0).then_some(d * step);
            jog(format!("Move {step} mm"), some(x), some(y), None)
        }
        "extrude" => one("Extrude 10 mm", Action::Extrude { mm: 10.0 }),
        "retract" => one("Retract 10 mm", Action::Extrude { mm: -10.0 }),
        "set-speed" => one("Speed", Action::Speed { percent: form.speed.ok_or("Speed: how much?")?.round() as u16 }),
        "set-flow" => one("Flow", Action::Flow { percent: form.flow.ok_or("Flow: how much?")?.round() as u16 }),
        "set-nozzle" => one("Nozzle", Action::Temperature { heater: Heater::Nozzle, celsius: form.nozzle.ok_or("Nozzle: how hot?")? }),
        "set-bed" => one("Bed", Action::Temperature { heater: Heater::Bed, celsius: form.bed.ok_or("Bed: how hot?")? }),
        "preheat-pla" => heat("Preheat PLA (215/60 °C)", 215.0, 60.0),
        "preheat-petg" => heat("Preheat PETG (240/80 °C)", 240.0, 80.0),
        "cooldown" => heat("Cool down", 0.0, 0.0),
        "fan-on" => one("Print fan on", Action::Fan { on: true }),
        "fan-off" => one("Print fan off", Action::Fan { on: false }),
        "motors-off" => one("Motors off", Action::MotorsOff),
        "load-filament" => one("Load filament", Action::LoadFilament { nozzle_c: spool.0, bed_c: spool.1 }),
        "unload-filament" => one("Unload filament", Action::UnloadFilament { nozzle_c: spool.0, bed_c: spool.1 }),
        "probe-bed" => one("Probe the bed", Action::ProbeBed),
        "unlock" => one("Unlock", Action::Unlock),
        "set-origin" => one("Set origin here", Action::SetOrigin),
        "go-origin" => one("Go to origin", Action::GoOrigin),
        "settings" => one("Read settings", Action::ReadSettings),
        _ => Err("no such command".to_owned()),
    }
}

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

/// HS6 (ours, in our UI): what the machine page's emergency-stop button does for a machine; the
/// reference of each is in `docs/briefs/hs6-emergency-stop.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyStop {
    /// An agent machine on the Marlin/OctoPrint port: the typed `M112`, which OctoPrint's comm
    /// turns into its own emergency stop (`util/comm.py:5524`, `:5487-5522`).
    TypedM112,
    /// A laser: LaserFoundry's own Stop, `Link::stop` (`ui-egui/src/panels.rs:774-775`), a soft
    /// reset. LaserFoundry has no other stop.
    LaserStop,
}

/// The emergency stop a machine has, if any. A label printer has none, and neither has a Prusa on
/// Buddy (`docs/briefs/hs6-emergency-stop.md`); the hub's own machines (built-in Connect,
/// SimplyPrint, Klipper) have none here.
pub fn emergency_stop_of(machine: &store::Machine) -> Option<EmergencyStop> {
    use protocol::MachineKind;
    if machine.kind == MachineKind::Laser {
        return Some(EmergencyStop::LaserStop);
    }
    store::on_marlin_port(machine).then_some(EmergencyStop::TypedM112)
}

/// The machine page's emergency-stop button.
pub(crate) async fn emergency_stop(
    access: crate::access::Access,
    State(shared): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let (space, machine) = {
        let store = shared.lock().unwrap();
        let space = match store.space_of(&access) {
            Ok((space, _)) => space,
            Err(error) => return failed(error),
        };
        match store.machine(space, &id) {
            Ok(Some(machine)) => (space, machine),
            Ok(None) => return (StatusCode::NOT_FOUND, "no such machine").into_response(),
            Err(error) => return failed(error),
        }
    };
    // Stopping never waits for PPE (`safety.rs`), and needs 'use' on the machine, as Stop does.
    match emergency_stop_of(&machine) {
        None => (StatusCode::CONFLICT, "this machine has no emergency stop").into_response(),
        Some(EmergencyStop::TypedM112) => {
            let queued = shared.lock().unwrap().queue_action(
                space,
                &id,
                "Emergency stop",
                &protocol::Action::Gcode { text: "M112".to_owned() },
            );
            match queued {
                Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
                Ok(None) => (StatusCode::NOT_FOUND, "no such machine, or it takes no commands").into_response(),
                Err(error) => failed(error),
            }
        }
        // The laser's Stop, the same route as its Stop button.
        Some(EmergencyStop::LaserStop) => {
            crate::files::stop_job(access, State(shared), axum::extract::Path(id)).await
        }
    }
}

pub(crate) async fn command(
    access: crate::access::Access,
    State(store): State<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Form(form): Form<CommandForm>,
) -> Response {
    let mut store = store.lock().unwrap();
    let space = match store.space_of(&access) {
        Ok((space, _)) => space,
        Err(error) => return failed(error),
    };
    // Switching things off never waits for PPE; everything else does.
    let off = form.gcode.trim().is_empty()
        && matches!(
            form.action.as_str(),
            "cooldown" | "fan-off" | "motors-off"
        );
    if !off && let Err(refused) = crate::safety::check_ppe(&store, &access, &id) {
        return *refused;
    }
    // B3: nothing here refuses a command for a machine behind an agent because of what it is doing
    // (not while it prints, say): the machine's port refuses as its reference does. Permissions and
    // PPE (above) are about people, and stay.
    // A machine behind an agent: typed commands, the bytes made by its port (the hub makes no
    // G-code for it, owner 2026-11-12).
    if let Ok(Some(machine)) = store.machine(space, &id)
        && machine.link == "agent"
    {
        let actions = if form.gcode.trim().is_empty() {
            agent_actions(&form, &machine)
        } else {
            typed_gcode(&form.gcode)
                .map(|(label, text)| vec![(label, protocol::Action::Gcode { text })])
                .map_err(|why| why.to_string())
        };
        let actions = match actions {
            Ok(actions) => actions,
            Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
        };
        for (label, action) in &actions {
            match store.queue_action(space, &id, label, action) {
                Ok(Some(_)) => {}
                Ok(None) => return (StatusCode::NOT_FOUND, "no such machine, or it takes no commands").into_response(),
                Err(error) => return failed(error),
            }
        }
        return if headers.contains_key("x-requested-with") {
            StatusCode::NO_CONTENT.into_response()
        } else {
            Redirect::to(&format!("/machines/{id}#control")).into_response()
        };
    }
    if !form.gcode.trim().is_empty() {
        let (label, gcode) = match typed_gcode(&form.gcode) {
            Ok(typed) => typed,
            Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
        };
        return match store.queue_command(space, &id, &label, &gcode) {
            Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
            Ok(None) => (
                StatusCode::NOT_FOUND,
                "no such machine, or it takes no commands",
            )
                .into_response(),
            Err(error) => failed(error),
        };
    }
    // A GRBL machine homes with $H, not G28.
    let grbl = || {
        store
            .machine(space, &id)
            .ok()
            .flatten()
            .and_then(|m| m.profile)
            .and_then(|p| profiles::find(&p))
            .and_then(|p| p.firmware.as_deref())
            == Some("grbl")
    };
    let owned = |(l, g): (&str, &str)| (l.to_owned(), g.to_owned());
    let is_grbl = grbl();
    // Machines behind an agent took the branch above; these are the hub's own (built-in
    // Connect, SimplyPrint, virtual), until they get the hub's internal agent.
    let found = if form.action.starts_with("jog") {
        jog_command(
            &form.action,
            form.step.unwrap_or(10.0),
            form.feed.unwrap_or(3000.0),
            is_grbl,
        )
    } else if is_grbl && let Some(found) = grbl_action(&form.action) {
        Some(owned(found))
    } else if let Some(found) = factor_command(&form.action, form.speed, form.flow) {
        match found {
            Ok(found) => Some(found),
            Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
        }
    } else if matches!(form.action.as_str(), "set-nozzle" | "set-bed") {
        let machine = match store.machine(space, &id) {
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
        let machine = match store.machine(space, &id) {
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
        let label = if load { "Load filament" } else { "Unload filament" };
        Some((
            format!("{label} ({hot:.0} °C)"),
            format!("M109 S{hot:.0}\n{}", if load { "M701" } else { "M702" }),
        ))
    } else if form.action == "probe-bed" {
        match store.machine(space, &id) {
            Ok(Some(machine)) => match probe_gcode(&machine) {
                Ok(gcode) => Some(owned(("Probe the bed", gcode))),
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
    if let Ok(Some(machine)) = store.machine(space, &id)
        && let Some(why) = refused_now(&form.action, &gcode, &machine, false)
    {
        return (StatusCode::CONFLICT, why).into_response();
    }
    let queued = store.queue_command(space, &id, &label, &gcode);
    match queued {
        // The page sends it without leaving; a plain form comes back to the page.
        Ok(Some(_)) if headers.contains_key("x-requested-with") => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Some(_)) => Redirect::to(&format!("/machines/{id}#control")).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "no such machine, or it takes no commands",
        )
            .into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::tests::{call, json};
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::Request;
    use maker_connect_protocol::{
        MachineKind, MachineState, Register, Registered, Telemetry, TelemetryReply,
    };
    use std::sync::{Arc, Mutex};

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

    /// Typed G-code is tidied and kept short enough for every machine.
    /// A machine behind an agent gets typed commands (`Action`), one at a time as it answers
    /// each, never G-code made by the hub; its SD tab says it is not available yet.
    #[tokio::test]
    async fn an_agents_machine_gets_typed_commands() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let agent = crate::app(store.clone());
        let register = Register {
            protocol: protocol::VERSION,
            name: "Printer".into(),
            kind: MachineKind::Fdm,
            agent_version: "0.2.0".into(),
        };
        let (_, body) = call(&agent, json(protocol::REGISTER, None, serde_json::to_string(&register).unwrap())).await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let id = registered.machine_id.clone();
        {
            let store = store.lock().unwrap();
            let space = store.default_space().unwrap().0;
            store.pair(space, &registered.pairing_code).unwrap();
            crate::auth::tester_may_operate(&store, space, &id);
        }
        let press = |body: &str| {
            Request::post(format!("/machines/{id}/command"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        for body in ["action=home", "action=x%2B&step=1&feed=1500", "action=preheat-pla", "gcode=M105"] {
            let (status, why) = call(&app, press(body)).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}: {why}");
        }
        let (status, _) = call(&app, press("action=x%2B&step=7")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a step not on the page's list");
        // Reported idle, answering each command as it comes: the commands, in order.
        let mut got = Vec::new();
        let mut done = Vec::new();
        for _ in 0..8 {
            let telemetry = Telemetry {
                state: MachineState::Idle,
                done: std::mem::take(&mut done),
                ..Telemetry::default()
            };
            let (_, body) = call(&agent, json(protocol::TELEMETRY, Some(&registered.token), serde_json::to_string(&telemetry).unwrap())).await;
            let reply: TelemetryReply = serde_json::from_str(&body).unwrap();
            for command in reply.commands {
                done.push(protocol::CommandDone { id: command.id, ok: true, reason: None, output: Vec::new() });
                got.push(command.action);
            }
        }
        use protocol::{Action, Axis, Heater};
        assert_eq!(
            got,
            [
                Action::Home { axes: vec![Axis::X, Axis::Y, Axis::Z] },
                Action::Jog { x: Some(1.0), y: None, z: None, feed: Some(1500.0) },
                Action::Temperature { heater: Heater::Nozzle, celsius: 215.0 },
                Action::Temperature { heater: Heater::Bed, celsius: 60.0 },
                Action::Gcode { text: "M105".into() },
            ]
        );
        let (_, sd) = call(&app, Request::get(format!("/api/machines/{id}/sd")).body(Body::empty()).unwrap()).await;
        assert!(sd.contains(crate::sd_card::NOT_YET), "{sd}");
    }

    /// B3: the hub does not hold or refuse commands for a machine behind an agent because of what it
    /// does (busy, running a job, waiting for someone): it queues them, one at a time, so typed G-code
    /// such as `M112` reaches the port at once (B7). B5: the two Home buttons of OctoPrint's panel.
    #[tokio::test]
    async fn b3_b5_an_agents_machine_gets_every_command_whatever_it_is_doing() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let agent = crate::app(store.clone());
        let register = Register {
            protocol: protocol::VERSION,
            name: "Printer".into(),
            kind: MachineKind::Fdm,
            agent_version: "0.2.0".into(),
        };
        let (_, body) = call(&agent, json(protocol::REGISTER, None, serde_json::to_string(&register).unwrap())).await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        let id = registered.machine_id.clone();
        {
            let store = store.lock().unwrap();
            let space = store.default_space().unwrap().0;
            store.pair(space, &registered.pairing_code).unwrap();
            crate::auth::tester_may_operate(&store, space, &id);
        }
        let press = |body: &str| {
            Request::post(format!("/machines/{id}/command"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-requested-with", "fetch")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        // what the page refused while it printed: typed G-code, the fan, Z, homing, extruding
        for body in ["gcode=M112", "action=fan-on", "action=z%2B&step=1", "action=home-xy", "action=home-z", "action=home", "action=extrude"] {
            let (status, why) = call(&app, press(body)).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}: {why}");
        }
        use protocol::{Action, Axis};
        let mut got = Vec::new();
        let mut done = Vec::new();
        // busy with a job, then waiting for someone (Attention): commands go out in both
        for round in 0..16 {
            let telemetry = Telemetry {
                state: if round < 8 { MachineState::Busy } else { MachineState::Attention },
                job_id: (round < 8).then_some(7),
                done: std::mem::take(&mut done),
                ..Telemetry::default()
            };
            let (_, body) = call(&agent, json(protocol::TELEMETRY, Some(&registered.token), serde_json::to_string(&telemetry).unwrap())).await;
            let reply: TelemetryReply = serde_json::from_str(&body).unwrap();
            for command in reply.commands {
                done.push(protocol::CommandDone { id: command.id, ok: true, reason: None, output: Vec::new() });
                got.push((round, command.action));
            }
        }
        let actions: Vec<Action> = got.iter().map(|(_, a)| a.clone()).collect();
        assert_eq!(
            actions,
            [
                Action::Gcode { text: "M112".into() },
                Action::Fan { on: true },
                Action::Jog { x: None, y: None, z: Some(1.0), feed: None },
                Action::Home { axes: vec![Axis::X, Axis::Y] },
                Action::Home { axes: vec![Axis::Z] },
                Action::Home { axes: vec![Axis::X, Axis::Y, Axis::Z] },
                Action::Extrude { mm: 10.0 },
            ]
        );
        assert!(got.iter().any(|(round, _)| *round < 8), "while it was busy");
        // A command for a machine that waits for someone goes too.
        let (status, why) = call(&app, press("gcode=M105")).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{why}");
        let telemetry = Telemetry { state: MachineState::Attention, ..Telemetry::default() };
        let (_, body) = call(&agent, json(protocol::TELEMETRY, Some(&registered.token), serde_json::to_string(&telemetry).unwrap())).await;
        let reply: TelemetryReply = serde_json::from_str(&body).unwrap();
        assert_eq!(reply.commands.len(), 1, "{body}");
    }

    /// HS6: an agent machine of each kind, registered, paired, with a first report (so it is online).
    async fn agent_of(
        store: &Arc<Mutex<Store>>,
        kind: MachineKind,
        profile: Option<&str>,
    ) -> (String, String) {
        let agent = crate::app(store.clone());
        let register = Register { protocol: protocol::VERSION, name: "m".into(), kind, agent_version: "0.2.0".into() };
        let (_, body) = call(&agent, json(protocol::REGISTER, None, serde_json::to_string(&register).unwrap())).await;
        let registered: Registered = serde_json::from_str(&body).unwrap();
        {
            let store = store.lock().unwrap();
            let space = store.default_space().unwrap().0;
            store.pair(space, &registered.pairing_code).unwrap();
            // 'use' only, not 'operate': whoever may use the machine may press the emergency stop
            let user: i64 = store.db.query_row("SELECT id FROM users WHERE name = 'tester'", [], |r| r.get(0)).unwrap();
            store.set_grant(space, &registered.machine_id, user, crate::access::Level::Use, user, None).unwrap();
            if profile.is_some() {
                store.set_profile(space, &registered.machine_id, profile).unwrap();
            }
        }
        let telemetry = Telemetry { state: MachineState::Idle, ..Telemetry::default() };
        call(&agent, json(protocol::TELEMETRY, Some(&registered.token), serde_json::to_string(&telemetry).unwrap())).await;
        (registered.machine_id, registered.token)
    }

    /// HS6: the commands the agent is given on its next report.
    async fn given(store: &Arc<Mutex<Store>>, token: &str) -> Vec<protocol::Action> {
        let agent = crate::app(store.clone());
        let telemetry = Telemetry { state: MachineState::Idle, ..Telemetry::default() };
        let (_, body) = call(&agent, json(protocol::TELEMETRY, Some(token), serde_json::to_string(&telemetry).unwrap())).await;
        serde_json::from_str::<TelemetryReply>(&body).unwrap().commands.into_iter().map(|c| c.action).collect()
    }

    /// HS6: the button's route, per kind: a Marlin machine is sent exactly the typed `M112` (the path
    /// `b3_b5_...` shows for the terminal), a laser is sent LaserFoundry's Stop (`JobControl::Stop`, the
    /// route of its Stop button); a label printer, a Prusa (Buddy) and a machine whose profile is not
    /// Marlin are refused and are sent nothing. The page shows the button for the first two only.
    #[tokio::test]
    async fn hs6_the_emergency_stop_is_each_kinds_own() {
        let store = Arc::new(Mutex::new(Store::open(":memory:").unwrap()));
        let app = crate::auth::signed_in_app(store.clone());
        let press = |id: &str| Request::post(format!("/machines/{id}/emergency-stop")).body(Body::empty()).unwrap();
        let page = |id: &str| Request::get(format!("/machines/{id}")).body(Body::empty()).unwrap();

        // a Marlin agent machine: the typed M112, and nothing else
        let (marlin, token) = agent_of(&store, MachineKind::Fdm, None).await;
        let (status, why) = call(&app, press(&marlin)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{why}");
        assert_eq!(given(&store, &token).await, [protocol::Action::Gcode { text: "M112".into() }]);
        assert!(call(&app, page(&marlin)).await.1.contains("emergency-stop"));
        // with 'use' alone, the terminal is refused: the button is a stop, which 'use' allows
        let typed = Request::post(format!("/machines/{marlin}/command"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("gcode=M105"))
            .unwrap();
        assert_eq!(call(&app, typed).await.0, StatusCode::FORBIDDEN);

        // a laser: LaserFoundry's Stop, to the agent as a job control
        let (laser, _) = agent_of(&store, MachineKind::Laser, None).await;
        let (status, why) = call(&app, press(&laser)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{why}");
        assert!(matches!(
            store.lock().unwrap().own_job_control.remove(&laser),
            Some(protocol::JobControl::Stop)
        ));
        // its page: one button, its Stop named the emergency stop, no second form
        let page_of_laser = call(&app, page(&laser)).await.1;
        assert!(!page_of_laser.contains("emergency-stop"), "not shown twice");
        assert!(page_of_laser.contains(">Emergency stop (soft reset)</button>"));

        // none: a label printer, a Prusa on Buddy, a profile that is not Marlin
        for (kind, profile) in [
            (MachineKind::Label, None),
            (MachineKind::Fdm, Some("prusa/mini")),
            (MachineKind::Fdm, Some("prusa/mk4s")),
        ] {
            let (id, token) = agent_of(&store, kind, profile).await;
            let (status, why) = call(&app, press(&id)).await;
            assert_eq!(status, StatusCode::CONFLICT, "{kind:?} {profile:?}: {why}");
            assert!(given(&store, &token).await.is_empty(), "{kind:?} {profile:?}: nothing sent");
            assert!(store.lock().unwrap().own_job_control.remove(&id).is_none());
            assert!(!call(&app, page(&id)).await.1.contains("emergency-stop"), "{kind:?} {profile:?}");
        }
    }
}
