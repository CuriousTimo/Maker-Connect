//! A machine's page: four tabs (overview, control, telemetry, settings) of tiles and cards,
//! rendered here and kept live by its script (`machine.js`) from the server-sent events.
//! Every value the script updates carries `data-live="<key>"`, rendered here the same way.

use crate::profiles::{self, Profile};
use crate::store::{self, Machine};
use crate::{STYLE, escape};
use maker_connect_protocol::MachineKind;

const MACHINE_STYLE: &str = include_str!("../machine.css");
const MACHINE_SCRIPT: &str = include_str!("../machine.js");

/// The design's badge class for a state class: as shell.js `ST`.
fn st_class(cls: &str) -> &'static str {
    match cls {
        "idle" | "cooling" | "loaded" => "idle",
        "busy" => "busy",
        "paused" | "attention" => "warn",
        "error" => "bad",
        "offline" | "disconnected" | "out" => "off",
        _ => "unk",
    }
}

/// How a machine's state is shown, and its badge's class: as the pages' `shownState` does.
/// A machine that is ready but waits for something (no cassette) shows that as a notice.
pub(crate) fn shown_state(machine: &Machine) -> (String, String) {
    if machine.out_of_use.is_some() {
        return ("Out of use".to_owned(), "out".to_owned());
    }
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    // A finished print on the bed: cooling, then it waits for someone to take it.
    if machine.on_bed.is_some()
        && let Some(t) = t.filter(|t| t.state == maker_connect_protocol::MachineState::Idle)
    {
        return if t.bed_c.is_some_and(|c| c > store::TAKE_BELOW_C) {
            ("Cooling".to_owned(), "cooling".to_owned())
        } else {
            ("Take the print".to_owned(), "attention".to_owned())
        };
    }
    match t {
        None => ("Offline".to_owned(), "offline".to_owned()),
        Some(t) => match (&t.problem, t.state) {
            (problem, maker_connect_protocol::MachineState::Attention) => (
                problem.clone().unwrap_or_else(|| "Attention".to_owned()),
                "attention".to_owned(),
            ),
            (Some(problem), maker_connect_protocol::MachineState::Idle) => {
                (problem.clone(), "attention".to_owned())
            }
            // HS7b: a laser with a job loaded, idle: it waits to be framed and started there.
            (None, maker_connect_protocol::MachineState::Idle)
                if machine
                    .job
                    .as_ref()
                    .is_some_and(|j| j.state == store::JobState::Loaded) =>
            {
                ("Loaded".to_owned(), "loaded".to_owned())
            }
            _ => (t.state.label().to_owned(), t.state.label().to_lowercase()),
        },
    }
}

/// What the page shows beyond the machine itself: whether who looks may manage it, and its
/// latest jobs with their numbers.
pub struct Extra {
    /// Its Camera tab (crate::cameras): the cameras on it the person may watch.
    pub camera_tab: Option<String>,
    /// Its first camera, for the Now tab beside this print.
    pub camera_now: Option<String>,
    /// Who looks at it, and whether they are staff (admin, owner): they may change
    /// everyone's prints in its queue; others only their own (crate::queue).
    pub me: i64,
    pub staff: bool,
    pub manage: bool,
    /// Whether the person may use it (jobs and more): by permission, whatever their role.
    pub may_use: bool,
    /// May they move, heat and send G-code ('operate', crate::access): without it the controls
    /// that send commands are hidden, and one notice says so (ui.md, the control rule).
    pub may_operate: bool,
    /// When they asked the staff for permission, while nobody answered.
    pub asked: Option<i64>,
    /// When they asked the staff to operate it, while nobody answered.
    pub asked_operate: Option<i64>,
    /// Until when they confirmed having and wearing its PPE (crate::safety).
    pub ppe_until: Option<i64>,
    /// Whether the space asks for PPE before use.
    pub ppe_checks: bool,
    /// Spaces it could be moved to: (uid, label).
    pub move_to: Vec<(String, String)>,
    pub jobs: Vec<store::Job>,
    pub stats: store::JobStats,
    /// HS7b: a laser's loaded job was framed completely after its last Set origin.
    pub laser_framed: bool,
    /// HS7b: a Set origin was done on this laser since the hub started.
    pub laser_origin_set: bool,
}

/// A machine without a profile: one from what it says of itself. A laser: its work area from
/// its controller; a Marlin printer on the agent: what its firmware says (M115, extension part 9).
fn told_profile(machine: &Machine) -> Option<Profile> {
    if crate::prusa_connect::marlin_agent(machine) {
        let info = machine.details.as_ref()?.firmware_info.as_ref()?;
        let mut profile = profiles::from_firmware(info);
        // M115 does not say the bed: the space's, when it said it (Settings, Profile).
        if let Some(bed) = machine.bed_mm {
            profile.build_volume_mm = bed;
        }
        return Some(profile);
    }
    let travel = machine.telemetry.as_ref()?.grbl.as_ref()?.travel_mm?;
    Some(profiles::from_controller(machine.kind, travel))
}

/// The whole page.
pub fn render(space: &str, machine: &Machine, extra: &Extra) -> String {
    let found = machine.profile.as_deref().and_then(profiles::find);
    let told = found.is_none().then(|| told_profile(machine)).flatten();
    let profile = found.or(told.as_ref());
    let (state, state_class) = shown_state(machine);
    let state_class = st_class(&state_class);
    let kind = profile.map_or(machine.kind.label(), |p| p.model.as_str());
    let link = match machine.link.as_str() {
        "prusa-connect" if crate::prusa_connect::marlin_agent(machine) => "Maker Connect agent",
        "prusa-connect" => "Built-in Prusa Connect",
        "virtual" => "Virtual, in the hub",
        "simplyprint" => "Klipper, through Moonraker",
        other => other,
    };
    let laser = machine.kind == MachineKind::Laser;
    let subtitle = if machine.name.to_lowercase().contains(&kind.to_lowercase()) {
        link.to_owned()
    } else {
        format!("{kind} · {link}")
    };
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name} · Maker Connect</title>
<link rel="icon" href="../logo.svg" type="image/svg+xml">
<style>
{STYLE}{MACHINE_STYLE}</style>
</head>
<body>
{header}
<main id="main" tabindex="-1" class="wide" data-id="{id}" data-me="{me}" data-staff="{staff}">
  <p class="crumbs"><a href="../machines">Machines</a> › {name}</p>
{head}
  {labels}
  {no_use}<div class="notices" data-notices>{notices}</div><div class="notices" data-question></div>
  <nav class="tabs" role="tablist">
    <a href="#now" role="tab">Now</a>
    {queue_tab}<a href="#controls" role="tab">Controls</a>
    {usb_tab}{camera_link}<a href="#machine" role="tab">Machine</a>
    <a href="#history" role="tab">History</a>
    {advanced_tab}</nav>
  <section class="tab" id="overview" role="tabpanel">
{overview}
  </section>
  <section class="tab" id="queue" role="tabpanel" hidden>
{queue}
  </section>
  <section class="tab" id="jobs" role="tabpanel" hidden>
{jobs}
  </section>
  <section class="tab" id="control" role="tabpanel" hidden>
{control}
<div class="grid-2 control-more">{filament}{sheet}</div>
  </section>
  <section class="tab" id="terminal" role="tabpanel" hidden>
{terminal}
  </section>
  <section class="tab" id="telemetry" role="tabpanel" hidden>
{telemetry}
  </section>
  {bed_level}
  {usb}
  {camera_panel}<section class="tab" id="settings" role="tabpanel" hidden>
{settings}
{upkeep}
  </section>
</main>
<script>
{MACHINE_SCRIPT}</script>
</body>
</html>
"##,
        name = escape(&machine.name),
        header = crate::header(space, "../", "machines"),
        id = escape(&machine.id),
        head = head(
            machine,
            profile,
            &state,
            state_class,
            &subtitle,
            extra.may_operate
        ),
        queue_tab = if machine.kind == maker_connect_protocol::MachineKind::Label {
            ""
        } else {
            r##"<a href="#queue" role="tab">Queue <small class="tab-count" data-queue-count></small></a>
    "##
        },
        notices = notices(machine),
        me = extra.me,
        staff = extra.staff,
        queue = if machine.kind == maker_connect_protocol::MachineKind::Label {
            String::new()
        } else {
            queue_card(machine)
        },
        no_use = before_use(machine, extra),
        overview = if heats(machine) || laser {
            now(machine, profile, extra)
        } else {
            overview(machine, profile, "")
        },
        jobs = jobs_card(machine, extra),
        labels = labels_line(machine, extra.manage),
        filament = filament_card(machine, extra.may_operate),
        sheet = sheet_card(machine),
        upkeep = upkeep_card(machine, extra.manage),
        control = control(
            machine,
            profile,
            &loaded_card(machine, extra),
            extra.may_operate,
            &no_operate(machine, extra),
        ),
        telemetry = telemetry_tab(machine),
        settings = settings(machine, &extra.move_to, extra.ppe_checks),
        // A laser has no console (decision 20261119-no-raw-gcode-on-a-laser): no Advanced tab.
        advanced_tab = if laser {
            ""
        } else {
            r##"<a href="#advanced" role="tab">Advanced</a>
  "##
        },
        terminal = if laser {
            String::new()
        } else if extra.may_operate || !store::takes_commands(&machine.link) {
            terminal(machine)
        } else {
            no_operate(machine, extra)
        },
        usb_tab = if machine.link == "prusa-connect" {
            r##"<a href="#files" role="tab">Files</a>
    "##
        } else {
            ""
        },
        usb = if machine.link == "prusa-connect" {
            storage_tab(machine)
        } else {
            String::new()
        },
        camera_link = if extra.camera_tab.is_some() {
            r##"<a href="#cameras" role="tab">Cameras</a>
    "##
        } else {
            ""
        },
        camera_panel = match &extra.camera_tab {
            Some(tab) => format!(
                r#"<section class="tab" id="camera" role="tabpanel" hidden>
<style>{}</style>{tab}
  </section>
  "#,
                crate::cameras_style()
            ),
            None => String::new(),
        },
        bed_level = if shows_bed_mesh(machine) {
            format!(
                r#"<section class="tab" id="bedlevel" role="tabpanel" hidden>
{}
  </section>"#,
                bed_level(machine, profile, extra.may_operate)
            )
        } else {
            String::new()
        },
    )
}

/// The head's and the machine list's icons (ui.md "Head", "The card"): our own drawings.
const ICON_NOZZLE: &str = r#"<svg class="ico" viewBox="0 0 20 20" aria-hidden="true"><path d="M5 3h10v6H5zM7 9h6l-2 5H9z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/><path d="M10 14v3" stroke="currentColor" stroke-width="1.6"/></svg>"#;
const ICON_BED: &str = r#"<svg class="ico" viewBox="0 0 20 20" aria-hidden="true"><path d="M3 15h14" stroke="currentColor" stroke-width="1.8"/><path d="M6 12c-1-1.5 1-2.5 0-4M10 12c-1-1.5 1-2.5 0-4M14 12c-1-1.5 1-2.5 0-4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>"#;
const ICON_SPEED: &str = r#"<svg class="ico" viewBox="0 0 20 20" aria-hidden="true"><path d="M3.5 14a6.5 6.5 0 0 1 13 0" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><path d="M10 14l3.5-4" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>"#;
const ICON_FLOW: &str = r#"<svg class="ico" viewBox="0 0 20 20" aria-hidden="true"><path d="M10 3c3 4 5 6.5 5 9a5 5 0 0 1-10 0c0-2.5 2-5 5-9z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/></svg>"#;

/// The page's head (Prusa Connect's printer head, our own build): the machine's picture; its
/// name, state and a pencil to rename it; what it is, its firmware and its material; for a
/// printer a row of live chips (nozzle, bed, speed, flow); and Set ready on the right.
fn head(
    machine: &Machine,
    profile: Option<&Profile>,
    state: &str,
    state_class: &str,
    subtitle: &str,
    may_operate: bool,
) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    // Who may operate a machine that takes commands sets a heater from its value (Prusa Connect's
    // head): Off, the firmware's preheat presets, a value of one's own; up to the profile's maximum.
    let settable = may_operate && t.is_some() && crate::store::takes_commands(&machine.link);
    let menu = |key: &str, max: f64, pick: PresetColumn, now: f64| {
        let item = |text: &str, celsius: f64| {
            format!(
                r#"<span class="heat-set"><input type="hidden" name="{key}" value="{celsius:.0}"><button name="action" value="set-{key}">{text}</button></span>"#
            )
        };
        let mut items = item("Off", 0.0);
        for preset in crate::commands::PREHEAT_PRESETS
            .iter()
            .filter(|p| pick(p) <= max)
        {
            items.push_str(&item(
                &format!("{} <span>{:.0} °C</span>", preset.0, pick(preset)),
                pick(preset),
            ));
        }
        format!(
            r#"<form class="command temp-menu" method="post" action="{id}/command">{items}<label class="heat-set">Other <input type="number" name="{key}" min="0" max="{max:.0}" step="1" value="{now:.0}" inputmode="numeric"> °C <button name="action" value="set-{key}" class="btn small">Set</button></label></form>"#,
            id = escape(&machine.id),
        )
    };
    let picture = profile
        .and_then(Profile::image)
        .or_else(|| profiles::generic_image(machine.kind))
        .unwrap_or(r#"<div class="no-picture">No picture</div>"#);
    // printer.html: one muted line, what it is and its firmware.
    let firmware = t
        .and_then(|t| t.firmware.clone())
        .or_else(|| machine.details.as_ref().and_then(|d| d.firmware.clone()))
        .map(|v| format!(r#" · <span data-live="firmware">{}</span>"#, escape(&v)))
        .unwrap_or_default();
    let material = if heats(machine) {
        let (swatch, what) = match &machine.loaded {
            Some(l) => (
                l.color
                    .as_deref()
                    .filter(|c| {
                        c.starts_with('#')
                            && c.len() <= 9
                            && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
                    })
                    .map(str::to_owned),
                escape(&l.material.clone().unwrap_or_else(|| l.name.clone())),
            ),
            None => (
                None,
                format!(
                    r#"<span data-live="material">{}</span>"#,
                    escape(
                        &t.and_then(|t| t.material.clone())
                            .or_else(|| machine.details.as_ref().and_then(|d| d.material.clone()))
                            .unwrap_or_else(|| "None set".to_owned())
                    )
                ),
            ),
        };
        format!(
            r#"<a class="st val" href="../stock" aria-label="Material" title="Set a spool in Materials"><span class="swatch" style="background:{}"></span>{what}<span aria-hidden="true">▾</span></a>"#,
            swatch.as_deref().unwrap_or("transparent")
        )
    } else {
        String::new()
    };
    let chips = if heats(machine) {
        // printer.html: "Nozzle 215 °C / 215", the target only while one is set.
        let heat = |label: &str,
                    key: &str,
                    icon: &str,
                    now: Option<f32>,
                    target: Option<f32>,
                    on: Option<bool>| {
            let value = format!(
                r#"{icon}<b><span data-live="{key}_n">{}</span> °C</b><span data-live="{key}_t">{}</span>{}"#,
                escape(&now.map_or("–".to_owned(), |v| format!("{v:.1}"))),
                target_note(target),
                flame(key, on),
            );
            if !settable {
                return format!(
                    r#"<span class="st val" title="{label}" aria-label="{label}">{value}</span>"#
                );
            }
            let (max, pick, fallback): (f64, PresetColumn, f64) = if key == "bed" {
                (
                    profile.and_then(|p| p.max_bed_c).unwrap_or(100.0),
                    |p| p.2,
                    60.0,
                )
            } else {
                (
                    profile.and_then(|p| p.max_nozzle_c).unwrap_or(260.0),
                    |p| p.1,
                    215.0,
                )
            };
            let now = target.filter(|v| *v > 0.0).map_or(fallback, f64::from);
            format!(
                r#"<details class="temp-pick"><summary class="st val" title="{label}: set its temperature" aria-label="{label}: set its temperature">{value} <span aria-hidden="true">▾</span></summary>{}</details>"#,
                menu(key, max, pick, now)
            )
        };
        let pct = |label: &str, key: &str, icon: &str, v: Option<f32>| {
            format!(
                r#"<span class="st val" title="{label}" aria-label="{label}">{icon}<b data-live="{key}">{}</b></span>"#,
                v.map_or("–".to_owned(), |v| format!("{v} %"))
            )
        };
        format!(
            "{}{}{}{}",
            heat(
                "Nozzle",
                "nozzle",
                ICON_NOZZLE,
                t.and_then(|t| t.nozzle_c),
                t.and_then(|t| t.nozzle_target_c),
                t.and_then(|t| t.nozzle_heating),
            ),
            heat(
                "Bed",
                "bed",
                ICON_BED,
                t.and_then(|t| t.bed_c),
                t.and_then(|t| t.bed_target_c),
                t.and_then(|t| t.bed_heating),
            ),
            pct(
                "Speed",
                "speed",
                ICON_SPEED,
                t.and_then(|t| t.speed_percent)
            ),
            pct("Flow", "flow", ICON_FLOW, t.and_then(|t| t.flow_percent)),
        )
    } else if machine.kind == MachineKind::Laser {
        laser_chips(machine, profile)
    } else {
        String::new()
    };
    // Set ready (Prusa Connect's head button): the print is taken off the bed, so the next
    // job may start; once ready, it can be taken back. Greyed out while there is nothing to say.
    let id = escape(&machine.id);
    let ready = if machine.kind == MachineKind::Scale {
        String::new()
    } else if machine.on_bed.is_some() {
        format!(
            r#"<form method="post" action="{id}/taken" data-async data-done="Ready" data-then="reload"><button class="ghost">Set ready</button></form>"#
        )
    } else if machine.ready.is_some() && machine.job.is_none() {
        format!(
            r#"<form method="post" action="{id}/not-ready" data-async data-done="Not ready" data-then="reload"><button class="ghost">Ready · undo</button></form>"#
        )
    } else {
        r#"<button class="ghost" disabled title="Ready once a finished print is taken off">Set ready</button>"#.to_owned()
    };
    format!(
        r##"<div class="mhead">
    <div class="mhead-pic picture">{picture}</div>
    <div class="mhead-main">
      <div class="mhead-title"><h1>{name}</h1><span class="st {state_class}" data-live="state">{state}</span>{chips}{material}<a class="rename" href="#machine" title="Rename" aria-label="Rename"><svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16v4zM14 6l4 4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg></a></div>
      <p class="muted">{subtitle}{firmware}</p>
    </div>
    <div class="mhead-act">{ready}</div>
  </div>"##,
        name = escape(&machine.name),
        state = escape(state),
        subtitle = escape(subtitle),
    )
}

/// The printer's storage on its Connect, filled by the page's script: a Prusa's USB stick, a
/// Marlin printer's agent storage (`docs/design/prusa-sdk-extension.md` part 9), the root
/// `printer_files::storage_root` gives.
fn storage_tab(machine: &Machine) -> String {
    let root = crate::printer_files::root_of(machine);
    // Buddy fetches it encrypted (START_ENCRYPTED_DOWNLOAD), the agent as the SDK does
    // (START_CONNECT_DOWNLOAD).
    let (name, how) = if crate::prusa_connect::marlin_agent(machine) {
        ("agent's storage", "")
    } else {
        ("USB stick", ", encrypted")
    };
    USB_TAB
        .replace("{root}", root)
        .replace("{how}", how)
        .replace("{name}", name)
        .replace("{Name}", &(name[..1].to_uppercase() + &name[1..]))
}

const USB_TAB: &str = r#"<section class="tab" id="usb" role="tabpanel" hidden>
  <div class="card usb" data-usb data-usb-root="{root}" data-usb-name="{name}">
    <div class="card-head"><div><h3 data-usb-crumbs>{Name}</h3><p class="card-sub" data-usb-status></p></div></div>
    <div class="usb-tools">
      <form data-usb-send class="usb-send"><select data-usb-file aria-label="A file from Files"><option value="">A file from Files…</option></select><button>Send here</button></form>
      <form data-usb-folder class="usb-new"><input name="name" required maxlength="60" pattern="[A-Za-z0-9 _\-]+" placeholder="New folder" aria-label="Name of a new folder"><button class="ghost">Make</button></form>
    </div>
    <div class="usb-list" data-usb-list><p class="empty">Waiting for the printer…</p></div>
    <p class="hint">Files come from the printer one by one, with the picture the slicer put in them. A file sent here is fetched by the printer itself{how}; printing a file from Files on this printer sends it here first.</p>
  </div>
</section>"#;

/// What someone should know before anything else, under the title: the machine is out of
/// use, or a finished print waits on its bed. As the page's script draws them, live.
fn notices(machine: &Machine) -> String {
    let id = escape(&machine.id);
    let mut out = String::new();
    if let Some(o) = &machine.out_of_use {
        let why = if o.reason.is_empty() {
            String::new()
        } else {
            format!(": {}", escape(&o.reason))
        };
        out += &format!(
            r#"<div class="notice out"><span><b>Out of use</b>{why}. It takes no jobs until it is back.</span><form method="post" action="{id}/out-of-use" data-async data-done="Back in use" data-then="reload"><input type="hidden" name="action" value="back"><button class="ghost">Back in use</button></form></div>"#
        );
    }
    if let Some(u) = machine.upkeep.as_ref().filter(|u| u.due) {
        out += &format!(
            r##"<div class="notice bed"><span><b>Service due</b>: {:.0} of {:.0} {}. It takes no work from the queue until it is done.</span><a href="#settings">Upkeep</a></div>"##,
            u.since_h,
            u.every_h,
            work_hours(machine.kind)
        );
    }
    // A plate for a project waits for someone: the same as the script draws (confirmHtml).
    if let Some(c) = &machine.confirm {
        let ok = c.state == "finished";
        out += &format!(
            r#"<div class="notice confirm"><span><b>{file}</b>{part} {how}</span>
      <form data-confirm action="{id}/ready"><input type="hidden" name="job" value="{job}">
        <label>Good <input name="good" type="number" min="0" max="10000" value="{good}" aria-label="Parts that came off good"> / {per}</label>
        <button>Set ready</button><button class="danger" formaction="{id}/bad">Bad print</button></form></div>"#,
            file = escape(&c.file_name),
            part = if c.part.is_empty() {
                String::new()
            } else {
                format!(
                    " for <b>{}</b>{}",
                    escape(&c.part),
                    if c.project.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", escape(&c.project))
                    }
                )
            },
            how = if ok {
                "is done: how many came off good?"
            } else if c.state == "failed" {
                "failed: did any come off good?"
            } else {
                "was stopped: did any come off good?"
            },
            job = c.job,
            good = if ok { c.per_plate } else { 0 },
            per = c.per_plate,
        );
    }
    if let Some(bed) = &machine.on_bed {
        let t = machine.telemetry.as_ref().filter(|_| machine.online);
        let warm = t.and_then(|t| t.bed_c).filter(|c| *c > store::TAKE_BELOW_C);
        let how = match warm {
            Some(c) => format!(
                "the bed cools ({c:.0} °C); take it at {:.0} °C or below",
                store::TAKE_BELOW_C
            ),
            None => "the bed is cool: take it off".to_owned(),
        };
        out += &format!(
            r#"<div class="notice bed"><span><b>{}</b> is done and on the bed: {how}. The next job waits until it is taken.</span><form method="post" action="{id}/taken" data-async data-done="Print taken" data-then="reload"><button>Print taken</button></form></div>"#,
            escape(&bed.file_name)
        );
    }
    // Ready for the next job, until it starts; taken back as PrusaLink's "cancel ready".
    if machine.on_bed.is_none()
        && machine.job.is_none()
        && let Some(ready) = &machine.ready
    {
        out += &format!(
            r#"<div class="notice ready"><span>Ready for the next job: <b>{}</b> was taken off.</span><form method="post" action="{id}/not-ready" data-async data-done="Not ready" data-then="reload"><button>Not ready after all</button></form></div>"#,
            escape(&ready.file_name)
        );
    }
    out
}

/// The bed level tab: the last mesh as numbers, and as a surface, a map and a table.
fn bed_level(machine: &Machine, profile: Option<&Profile>, operate: bool) -> String {
    let mesh = machine.mesh.as_ref();
    let range = mesh.and_then(|m| {
        maker_connect_protocol::Mesh {
            rows: m.rows.clone(),
        }
        .range()
    });
    let mm = |v: f32| format!("{v:+.3}");
    let tiles = tile(
        "Range",
        "mesh_range",
        range.map(|(low, high)| format!("{:.3}", high - low)),
        "mm",
        "none",
        "highest minus lowest".to_owned(),
    ) + &tile(
        "Lowest",
        "mesh_low",
        range.map(|(low, _)| mm(low)),
        "mm",
        "none",
        "below where it should be".to_owned(),
    ) + &tile(
        "Highest",
        "mesh_high",
        range.map(|(_, high)| mm(high)),
        "mm",
        "none",
        "above where it should be".to_owned(),
    ) + &tile(
        "Measured",
        "mesh_age",
        mesh.map(|m| ago(store::now() - m.taken_at)),
        "",
        "mesh_points",
        mesh.map_or("never".to_owned(), |m| {
            format!(
                "{} × {} points",
                m.rows.first().map_or(0, Vec::len),
                m.rows.len()
            )
        }),
    );
    let probe = match crate::probe_gcode(machine) {
        Ok(_) if store::takes_commands(&machine.link) && operate => format!(
            r#"<form class="command" method="post" action="{}/command"><button name="action" value="probe-bed">Probe bed</button></form>"#,
            escape(&machine.id)
        ),
        Ok(_) => String::new(),
        Err(why) => format!(r#"<p class="card-sub">{}</p>"#, escape(why)),
    };
    let [width, depth, _] = profile
        .map(|p| p.build_volume_mm)
        .filter(|v| v[0] > 0.0)
        .unwrap_or([200.0; 3]);
    let data = serde_json::to_string(&mesh).unwrap_or_else(|_| "null".to_owned());
    format!(
        r#"<div class="tiles">{tiles}</div>
<div class="card">
  <div class="card-head">
    <div><h3>Bed mesh</h3><p class="card-sub">How far each point is from where the machine expects the bed · seen from the front · drag to turn</p></div>
    <div class="views" role="group" aria-label="View">
      <button type="button" class="ghost" data-mesh-view="surface" aria-pressed="true">3D</button>
      <button type="button" class="ghost" data-mesh-view="map" aria-pressed="false">Map</button>
      <button type="button" class="ghost" data-mesh-view="table" aria-pressed="false">Table</button>
    </div>
  </div>
  <div class="mesh-scale"><span data-scale-low>–</span><i></i><span data-scale-high>–</span><small data-exaggeration></small></div>
  <div class="mesh-empty" data-mesh-empty{empty_hidden}>No mesh yet.</div>
  <canvas id="mesh3d" data-bed-width="{width}" data-bed-depth="{depth}" aria-label="The bed mesh as a surface"></canvas>
  <div data-mesh-map hidden></div>
  <div class="table-wrap" data-mesh-table hidden></div>
  <div class="probe">{probe}<p class="note" data-command-status role="status"></p></div>
</div>
<script type="application/json" id="mesh-data">{data}</script>"#,
        empty_hidden = if mesh.is_some() { " hidden" } else { "" },
    )
}

/// "12 s ago", "5 min ago", "3 h ago", "2 days ago".
pub(crate) fn ago(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s ago"),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// A scale's live line (Step 3e, E11): the status in usbscale's words (none for 4, where usbscale
/// prints the weight), then the weight and the unit as the scale says them.
pub(crate) fn scale_line(reading: &maker_connect_protocol::ScaleReading) -> String {
    let weight = format!("{} {}", reading.weight, reading.unit_name);
    if reading.status_text.is_empty() {
        weight
    } else {
        format!("{} · {weight}", reading.status_text)
    }
}

fn heats(machine: &Machine) -> bool {
    machine.kind == MachineKind::Fdm
}

/// Whether the page has the bed mesh (its view and Probe bed): a printer that heats, and not one
/// on Prusa Connect, whose protocol carries no mesh. Buddy's Connect client sends none
/// (Prusa-Firmware-Buddy v8.0.2 `src/connect`: no mesh in its telemetry, `render.cpp:124-306`,
/// nor in its events), and Prusa's server keeps none (`reference/prusa-connect`
/// `internal/connector/xbuddy/inbound.go:136-159`, the telemetry it keeps). Ours: hiding the
/// section (the server has no page to hide it on).
pub(crate) fn shows_bed_mesh(machine: &Machine) -> bool {
    if crate::prusa_connect::marlin_agent(machine) {
        // A Marlin printer on the agent: when its M115 says it keeps a mesh (`Cap:LEVELING_DATA`,
        // extension part 9; ui.md "Bed level").
        return machine
            .details
            .as_ref()
            .is_some_and(|d| d.can("LEVELING_DATA"));
    }
    heats(machine) && machine.link != "prusa-connect"
}

/// A tile: a label, a big live value with its unit, and a live line below.
fn tile(
    label: &str,
    key: &str,
    value: Option<String>,
    unit: &str,
    sub_key: &str,
    sub: String,
) -> String {
    format!(
        r#"<div class="tile"><div class="label">{label}</div><div class="value"><span data-live="{key}">{}</span><small>{unit}</small></div><div class="sub ticker"><span data-live="{sub_key}">{}</span></div></div>"#,
        escape(value.as_deref().unwrap_or("–")),
        escape(&sub)
    )
}

/// A battery's tile: how full, as the machine says it, and a battery that fills, green,
/// amber under 30 %, red under 15 % (the label page's script keeps it live).
fn battery_tile(percent: Option<u8>) -> String {
    let level = match percent {
        None => "none",
        Some(p) if p < 15 => "low",
        Some(p) if p < 30 => "mid",
        Some(_) => "ok",
    };
    let fill = f32::from(percent.unwrap_or(0)) * 19.0 / 100.0;
    format!(
        r#"<div class="tile"><div class="label">Battery</div><div class="value battery-value"><svg class="battery" data-battery data-level="{level}" viewBox="0 0 28 14" aria-hidden="true"><rect x="1" y="1" width="23" height="12" rx="2.5" class="shell"/><rect x="25" y="4.5" width="2" height="5" rx="1" class="nub"/><rect x="3" y="3" width="{fill:.1}" height="8" rx="1.2" class="charge" data-battery-fill/></svg><span data-live="battery_pct">{pct}</span><small>%</small></div><div class="sub" data-live="battery_note">{note}</div></div>"#,
        pct = percent.map_or("–".to_owned(), |p| p.to_string()),
        note = if percent.is_some() {
            "asked every 15 s"
        } else {
            "not reported"
        },
    )
}

/// A laser's head chips (laser.html): X and Y where the controller says the head is, live, and
/// the laser's optical power (as said under Machine, else its profile's).
fn laser_chips(machine: &Machine, profile: Option<&Profile>) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let axis = |name: &str, key: &str, what: &str, i: usize| {
        format!(
            r#"<span class="st val" title="{name}, {what}" aria-label="{name}"><small>{name}</small><b data-live="{key}">{}</b> mm</span>"#,
            t.and_then(|t| t.position_mm)
                .and_then(|p| p[i])
                .map_or("–".to_owned(), |v| format!("{v:.1}"))
        )
    };
    let power = machine
        .laser_w
        .or(profile.and_then(|p| p.laser_w))
        .map_or(String::new(), |w| {
            format!(
                r#"<span class="st val" title="Optical power" aria-label="Laser"><small>Laser</small><b>{w} W</b></span>"#
            )
        });
    axis("X", "pos_x", "from the left", 0) + &axis("Y", "pos_y", "from the front", 1) + &power
}

/// Which of a preset's temperatures a heater takes (its nozzle's or its bed's).
type PresetColumn = fn(&(&str, f64, f64)) -> f64;

/// A head chip's target: " / 215" while one is set (printer.html), else nothing.
fn target_note(target: Option<f32>) -> String {
    match target {
        Some(target) if target > 0.0 => format!(" / {target:.0}"),
        _ => String::new(),
    }
}

/// A heater's line under its temperature: what it heats to, and how it is getting there.
/// (Whether it has power now is the flame beside its name.) As the page's script says it.
fn heat_note(now: Option<f32>, target: Option<f32>) -> String {
    let Some(now) = now else {
        return "not reported".to_owned();
    };
    match target {
        Some(target) if target > 0.0 => {
            if (now - target).abs() < 2.0 {
                format!("target {target:.0} °C")
            } else if now < target {
                format!("▲ heating to {target:.0} °C")
            } else {
                format!("▼ cooling to {target:.0} °C")
            }
        }
        Some(_) if now > 40.0 => "target off · ▼ cooling".to_owned(),
        Some(_) => "target off".to_owned(),
        None if now > 40.0 => "no target told · ▼ cooling".to_owned(),
        None => "no target told".to_owned(),
    }
}

/// A heater's tile: its temperature, a flame beside its name while it has power (outlined
/// while it has none, none when the machine does not tell), what it heats to below.
fn heater_tile(
    label: &str,
    key: &str,
    now: Option<f32>,
    target: Option<f32>,
    heating: Option<bool>,
) -> String {
    format!(
        r#"<div class="tile"><div class="label heat-label">{label}{}</div><div class="value"><span data-live="{key}_n">{}</span><small>°C</small></div><div class="sub" data-live="{key}_note">{}</div></div>"#,
        flame(key, heating),
        escape(&now.map_or("–".to_owned(), |v| format!("{v:.1}"))),
        escape(&heat_note(now, target)),
    )
}

/// The flame of a heater: `data-on` says whether it has power ("" when not told).
fn flame(key: &str, heating: Option<bool>) -> String {
    let (on, said) = match heating {
        Some(true) => ("true", "Heater on"),
        Some(false) => ("false", "Heater off"),
        None => ("", ""),
    };
    format!(
        r#"<span class="flame" data-heater="{key}" data-on="{on}" role="img" aria-label="{said}" title="{said}"><svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M12 2.5c.6 3.2 2.6 4.9 4.2 6.9 1.4 1.8 2.3 3.6 2.3 5.8A6.5 6.5 0 0 1 12 21.7a6.5 6.5 0 0 1-6.5-6.5c0-2.3 1-4.2 2.6-5.7.2 1.6.9 2.8 2.1 3.5-.4-3.9.5-7.3 1.8-10.5z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg></span>"#
    )
}

/// The tiles every tab can show: temperatures for a printer, position for a laser.
fn heater_tiles(machine: &Machine) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    heater_tile(
        "Nozzle",
        "nozzle",
        t.and_then(|t| t.nozzle_c),
        t.and_then(|t| t.nozzle_target_c),
        t.and_then(|t| t.nozzle_heating),
    ) + &heater_tile(
        "Bed",
        "bed",
        t.and_then(|t| t.bed_c),
        t.and_then(|t| t.bed_target_c),
        t.and_then(|t| t.bed_heating),
    ) + &t
        // A chamber or enclosure (Prusa's CORE One, the XL's enclosure), where it tells.
        .filter(|t| t.chamber_c.is_some())
        .map(|t| heater_tile("Chamber", "chamber", t.chamber_c, t.chamber_target_c, None))
        .unwrap_or_default()
        // Each tool, where it has several (the XL's heads, an INDX's tools).
        + &t
            .filter(|t| !t.tools.is_empty())
            .map(|t| {
                format!(
                    r#"<div class="tile"><div class="label">Tools</div><div class="sub" data-live="tools">{}</div></div>"#,
                    escape(&tools_line(&t.tools))
                )
            })
            .unwrap_or_default()
}

/// "1 ▸ 215/215 · 2 30 · 3 off": each tool's nozzle, the active one marked.
fn tools_line(tools: &[maker_connect_protocol::ToolHeat]) -> String {
    tools
        .iter()
        .map(|tool| {
            let now = tool.nozzle_c.map_or("–".to_owned(), |c| format!("{c:.0}"));
            let target = tool
                .nozzle_target_c
                .filter(|c| *c > 0.0)
                .map(|c| format!("/{c:.0}"))
                .unwrap_or_default();
            let mark = if tool.active { " ▸" } else { "" };
            format!("{}{mark} {now}{target} °C", tool.number)
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn position_tiles(machine: &Machine) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let axis = |i: usize| {
        t.and_then(|t| t.position_mm)
            .and_then(|p| p[i])
            .map(|v| format!("{v:.1}"))
    };
    tile(
        "X",
        "pos_x",
        axis(0),
        "mm",
        "none",
        "from the left".to_owned(),
    ) + &tile(
        "Y",
        "pos_y",
        axis(1),
        "mm",
        "none",
        "from the front".to_owned(),
    )
}

/// "1 h 02 min", "12 min", "45 s".
fn duration(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s => format!("{} h {:02} min", s / 3600, s % 3600 / 60),
    }
}

/// How far the machine's job is: percent done, and a line about it. The script says the
/// same from the live list.
/// A job now: how far (percent), what (its state and file; this line scrolls when long),
/// and how long still (top right, never scrolled; machine.js adds the clock).
fn job_now(machine: &Machine) -> (Option<String>, String, String) {
    let progress = machine.telemetry.as_ref().and_then(|t| t.job.as_ref());
    let left = progress
        .and_then(|p| p.remaining_s)
        .map(|s| format!("{} left", duration(s)))
        .unwrap_or_default();
    let Some(job) = &machine.job else {
        return (None, "no job".to_owned(), String::new());
    };
    let percent = progress
        .filter(|p| p.total > 0)
        .map(|p| format!("{:.0}", p.done as f64 / p.total as f64 * 100.0));
    let state = serde_json::to_value(job.state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    (percent, format!("{state} · {}", job.file_name), left)
}

fn job_tile(machine: &Machine) -> String {
    let (percent, note, left) = job_now(machine);
    format!(
        r#"<div class="tile"><div class="tile-head"><div class="label">Job</div><span class="corner" data-live="job_eta">{}</span></div><div class="value"><span data-live="job_pct">{}</span><small>%</small></div><div class="sub ticker"><span data-live="job_note">{}</span></div></div>"#,
        escape(&left),
        escape(percent.as_deref().unwrap_or("–")),
        escape(&note)
    )
}

/// HS7b (owner, 2026-11-17): a laser's loaded job: sent to the laser and ready, not started. The
/// person frames it, sets the origin and frames again where needed, jogs, then presses Start burn
/// (`files.rs` `start_burn` checks it all again). Ours: the card, its texts and the confirm.
fn loaded_card(machine: &Machine, extra: &Extra) -> String {
    let Some(job) = machine
        .job
        .as_ref()
        .filter(|j| j.state == store::JobState::Loaded)
    else {
        // Nothing loaded: a laser the person may use links to the laser design tool (Prepare), when
        // this hub has it. The tool itself is not on the machine page.
        if machine.kind == maker_connect_protocol::MachineKind::Laser
            && extra.may_use
            && !crate::laser_design::ASSETS.is_empty()
        {
            return r#"<div class="card loaded-card"><div class="card-head"><div class="card-title"><h3>Loaded job</h3><p class="card-sub">Nothing loaded</p></div></div><p>Load a file from <a href="../files">Files</a>, or design one: <a href="../laser-design/" target="_blank" rel="noopener">Laser design</a> opens in a new tab (also under <a href="../prepare">Prepare</a>).</p></div>"#.to_owned();
        }
        return String::new();
    };
    let id = escape(&machine.id);
    let framed = if extra.laser_framed {
        "Framed"
    } else if extra.laser_origin_set {
        "Not framed since the last Set origin"
    } else {
        "Not framed yet"
    };
    let confirm = if extra.laser_origin_set {
        crate::files::ORIGIN_CONFIRM.to_owned()
    } else {
        crate::files::start_confirm(&job.file_name)
    };
    format!(
        r#"<div class="card loaded-card">
  <div class="card-head"><div class="card-title"><h3>Loaded job</h3><p class="card-sub">Sent to the laser, not started</p></div></div>
  <p><b>{name}</b>{by}</p>
  <ol class="loaded-steps">
    <li>Frame it: the head runs its outline with the laser off. <b data-loaded-framed>{framed}</b></li>
    <li>Set origin and jog where it has to go, then frame again (after a Set origin it must be framed again).</li>
    <li>Start burn, at the laser, glasses on.</li>
  </ol>
  <div class="buttons">
    <form method="post" action="../files/{file}/frame" data-async data-done="Framing"><input type="hidden" name="machine" value="{id}"><button class="ghost">Frame</button></form>
    <form method="post" action="{id}/start-burn" data-async data-done="Burning" data-then="reload" data-confirm="{confirm}" data-confirm-ok="Start burn" data-confirm-danger><input type="hidden" name="confirm" value="true"><button class="danger" data-start-burn{disabled}>Start burn</button></form>
  </div>
  <form method="post" action="{id}/job" data-async data-done="Taken off" data-then="reload" data-confirm="Take {name} off the laser? It was never started."><input type="hidden" name="action" value="unload"><button class="ghost">Take it off</button></form>
  <p class="hint">Another file sent to this laser replaces this one only after you confirm.</p>
</div>"#,
        name = escape(&job.file_name),
        by = job.by.as_deref().map_or(String::new(), |b| format!(
            " <small>loaded by {}</small>",
            escape(b)
        )),
        file = escape(&job.file_id),
        framed = framed,
        confirm = escape(&confirm),
        disabled = if extra.laser_framed {
            ""
        } else {
            " disabled title=\"Frame the loaded job first\""
        },
    )
}

/// The job card: the file, how far, how long, and pause, resume, stop.
fn job_card(machine: &Machine) -> String {
    let job = machine.job.as_ref();
    let progress = machine.telemetry.as_ref().and_then(|t| t.job.as_ref());
    let (percent, note, left) = job_now(machine);
    // A print the printer runs itself (its USB stick or SD card, or started at its screen):
    // no job of the hub's, but stopped, paused and resumed from here all the same.
    let own = job.is_none()
        && progress.is_some()
        && machine.telemetry.as_ref().is_some_and(|t| {
            matches!(
                t.state,
                maker_connect_protocol::MachineState::Busy
                    | maker_connect_protocol::MachineState::Paused
                    | maker_connect_protocol::MachineState::Attention
            )
        });
    let laser = machine.kind == maker_connect_protocol::MachineKind::Laser;
    // HS7b: a loaded laser job is not running: the "Loaded job" card shows it, not this one.
    let loaded = job.is_some_and(|j| j.state == store::JobState::Loaded);
    let running = (job.is_some() && !loaded) || own;
    let width = percent.clone().unwrap_or_else(|| "0".to_owned());
    let time = |s: Option<u64>| s.map_or("–".to_owned(), duration);
    let thumbnail = job.map_or(String::new(), |j| {
        format!(
            r#"<img class="job-thumb" data-job-thumb src="../files/{}/thumbnail.png" alt="" onerror="this.hidden=true">"#,
            escape(&j.file_id)
        )
    });
    format!(
        r#"<div class="card job-card">
  <div class="card-head"><div class="card-title"><h3>This print</h3><p class="card-sub" data-live="job_state">{state}</p></div><span class="corner" data-live="job_eta">{left}</span></div>
  <div class="job" data-job{hidden}>
    {thumbnail}
    <div class="job-main">
      <b data-live="job_name">{name}</b>
      <div class="progress"><i data-job-bar style="width: {width}%"></i></div>
      <div class="job-times">
        <span><small>Done</small><b data-live="job_done">{done}</b></span>
        <span><small>Elapsed</small><b data-live="job_elapsed">{elapsed}</b></span>
        <span><small>Left</small><b data-live="job_remaining">{remaining}</b></span>
      </div>
    </div>
  </div>
  <form class="job-actions" data-job-actions action="{id}/job"{actions_hidden}>
    <button name="action" value="pause" data-job-only{only_hidden}>Pause</button>
    <button name="action" value="resume" data-job-only{only_hidden}>Resume</button>
    <button name="action" value="stop" class="danger">{stop_label}</button>
  </form>
  <p class="empty no-job" data-no-job{shown}>{no_job}</p>
  <p class="hint" data-laser-stop{stop_hint_hidden}>Emergency stop halts the laser at once (LaserFoundry's Stop, a soft reset): motion stops and the laser goes off.</p>
  <div class="resume" data-resume{resume_hidden}>
    <p><b>Interrupted:</b> <span data-live="resume_name">{resume_name}</span> ({resume_why}). It can start again from line <b data-live="resume_line">{resume_line}</b>, a little before where it stopped; the head goes there with the laser off first.</p>
    <form data-restart action="{id}/job"><button name="action" value="restart">Start again from there</button></form>
  </div>
</div>"#,
        // Its state only: the file is in the card itself.
        state = escape(note.split(" · ").next().unwrap_or_default()),
        left = escape(&left),
        no_job = if laser {
            r#"Nothing loaded. <a href="../files">Load a file</a> on this laser from Files."#
        } else {
            r#"No job. <a href="../files">Print a file</a> on this machine."#
        },
        hidden = if running { "" } else { " hidden" },
        // Only while a job runs: with no job or a loaded one the hub has nothing to stop.
        actions_hidden = if running { "" } else { " hidden" },
        // A loaded job is shown in its own card above, so no "nothing loaded" here.
        shown = if running || loaded { " hidden" } else { "" },
        only_hidden = if running { "" } else { " hidden" },
        stop_hint_hidden = if laser && running { "" } else { " hidden" },
        stop_label = if laser {
            "Emergency stop (soft reset)"
        } else {
            "Stop"
        },
        name = escape(match job {
            Some(j) => j.file_name.as_str(),
            None if own => progress
                .map(|p| p.file.as_str())
                .filter(|f| !f.is_empty())
                .unwrap_or("a print the printer runs itself"),
            None => "",
        }),
        done = percent.map_or("–".to_owned(), |p| format!("{p} %")),
        elapsed = time(progress.and_then(|p| p.elapsed_s)),
        remaining = time(progress.and_then(|p| p.remaining_s)),
        id = escape(&machine.id),
        resume_hidden = if machine.resumable.is_some() {
            ""
        } else {
            " hidden"
        },
        resume_name = escape(
            machine
                .resumable
                .as_ref()
                .map_or("", |j| j.file_name.as_str())
        ),
        resume_why = escape(
            machine
                .resumable
                .as_ref()
                .and_then(|j| j.reason.as_deref())
                .unwrap_or("stopped")
        ),
        resume_line = machine
            .resumable
            .as_ref()
            .and_then(|j| j.restart_line)
            .unwrap_or(0),
    )
}

/// A card: a title, a line under it, and what it holds.
fn card(title: &str, sub: &str, body: &str) -> String {
    format!(
        r#"<div class="card"><div class="card-head"><div><h3>{title}</h3>{}</div></div>{body}</div>"#,
        if sub.is_empty() {
            String::new()
        } else {
            format!(r#"<p class="card-sub">{sub}</p>"#)
        }
    )
}

/// A row of a definition list; nothing when there is no value.
fn row(label: &str, value: Option<String>) -> String {
    value.map_or(String::new(), |v| {
        format!("<dt>{label}</dt><dd>{}</dd>", escape(&v))
    })
}

/// Its MMU as the printer says it: the slot in use (1-based, as on its screen) and what
/// it is doing.
fn mmu_text(t: Option<&maker_connect_protocol::Telemetry>) -> Option<String> {
    let t = t?;
    match (t.mmu_slot, t.mmu_doing.as_deref()) {
        (None, None) => None,
        (slot, doing) => Some(
            [
                slot.map(|s| format!("slot {}", s + 1)),
                doing.map(str::to_owned),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
        ),
    }
}

/// The machine's profile as shown: its model, and whether it was recognised or chosen.
fn profile_name(machine: &Machine, p: &Profile) -> String {
    format!(
        "{}{}",
        p.model,
        if p.id.is_empty() {
            ": $130 to $132; choose a profile under Settings"
        } else if machine.profile_chosen {
            ""
        } else {
            " (recognised)"
        }
    )
}

/// Everything known of what the machine is: its profile's facts, and what it told about
/// itself (over Prusa Connect, or through its agent). For Settings.
/// What it is (from its profile) and what it told about itself: the Machine tab's Details.
fn machine_facts(machine: &Machine, profile: Option<&Profile>) -> (String, String) {
    let t = machine.telemetry.as_ref();
    let details = machine.details.clone().unwrap_or_default();
    let identity = t.and_then(|t| t.identity.as_ref());
    let label = machine.kind == MachineKind::Label;
    let from_profile = profile.map_or(String::new(), |p| {
        [
            row("Profile", Some(profile_name(machine, p))),
            row(
                if label {
                    "Takes"
                } else if p.build_volume_mm[2] > 0.0 {
                    "Build volume"
                } else {
                    "Work area"
                },
                (label || p.build_volume_mm[0] > 0.0).then(|| p.size()),
            ),
            row("Resolution", p.dpi.map(|d| format!("{d} dpi"))),
            row("Nozzle size", p.nozzle_mm.map(|n| format!("{n} mm"))),
            row("Extruders", p.extruders.map(|n| n.to_string())),
            row(
                "Laser",
                machine.laser_w.or(p.laser_w).map(|w| format!("{w} W")),
            ),
            row(
                "Max speed",
                p.max_speed_mm_min.map(|v| format!("{v} mm/min")),
            ),
            row("Max nozzle", p.max_nozzle_c.map(|c| format!("{c} °C"))),
            row("Max bed", p.max_bed_c.map(|c| format!("{c} °C"))),
        ]
        .concat()
    });
    let told = [
        row(
            "Firmware",
            details
                .firmware
                .clone()
                .or_else(|| t.and_then(|t| t.firmware.clone())),
        ),
        // As the printer says it (a Prusa on Connect: its INFO; Prusa's MK3 through an agent:
        // PRUSA SN, M862.1 Q, its MMU's M707, its EEPROM's counters).
        // A device on the agent: its `sn` is the agent's, shown with the connection.
        row(
            "Serial number",
            details
                .serial
                .clone()
                .filter(|_| !on_agent(machine))
                .or_else(|| identity.and_then(|i| i.serial.clone())),
        ),
        row("Printer type", details.printer_type.clone()),
        row(
            "Nozzle in it",
            details.nozzle_mm.map(|n| format!("{n} mm")).or_else(|| {
                identity
                    .and_then(|i| i.nozzle_mm.clone())
                    .map(|n| format!("{n} mm"))
            }),
        ),
        row(
            "MMU firmware",
            identity.and_then(|i| i.mmu_firmware.clone()),
        ),
        row(
            "Filament, all time",
            t.and_then(|t| t.total_filament_m).map(|m| format!("{m} m")),
        ),
        row(
            "Printing, all time",
            t.and_then(|t| t.total_print_min)
                .map(|m| format!("{} h {:02} min", m / 60, m % 60)),
        ),
        row(
            "Status bytes",
            t.filter(|_| machine.online)
                .and_then(|t| t.raw_status.clone()),
        ),
    ]
    .concat()
        + &m115(&details);
    (from_profile, told)
}

/// What a Marlin printer answered M115 (extension part 9): each field of its first line as it
/// gave it, then its `Cap:` lines, those it supports and those it does not.
fn m115(details: &store::Details) -> String {
    let fields: String = details
        .firmware_info
        .iter()
        .flatten()
        // the key is the printer's: escaped, as `row` escapes the value
        .map(|(key, value)| row(&escape(key), value.as_str().map(str::to_owned)))
        .collect();
    let caps = |on: bool| {
        let names: Vec<&str> = details
            .firmware_capabilities
            .iter()
            .flatten()
            .filter(|(_, v)| v.as_bool() == Some(on))
            .map(|(k, _)| k.as_str())
            .collect();
        (!names.is_empty()).then(|| names.join(", "))
    };
    fields + &row("Supports", caps(true)) + &row("Does not support", caps(false))
}

/// A row the script keeps live; `–` while unknown.
fn live_row(label: &str, key: &str, value: Option<String>) -> String {
    format!(
        r#"<dt>{label}</dt><dd data-live="{key}">{}</dd>"#,
        escape(value.as_deref().unwrap_or("–"))
    )
}

/// Its labels under the title; who may manage it edits them there.
fn labels_line(machine: &Machine, manage: bool) -> String {
    let tags: String = machine
        .labels
        .iter()
        .map(|l| format!(r#"<span class="tag">{}</span>"#, escape(l)))
        .chain(machine.induction_required.then(|| {
            if machine.training {
                r#"<span class="tag induction" title="Only who had the training may use it">training</span>"#.to_owned()
            } else {
                r#"<span class="tag induction" title="Only who has permission may use it">permission</span>"#.to_owned()
            }
        }))
        .collect();
    let edit = if manage {
        format!(
            r#"<details class="label-edit"><summary>{}</summary><form method="post" action="{id}/labels" data-async data-then="reload" data-done="Labels saved"><input name="labels" value="{value}" maxlength="300" placeholder="MK4-BL, Room 2" aria-label="Labels, separated by commas"><button>Save</button></form></details>"#,
            if machine.labels.is_empty() {
                "Add labels"
            } else {
                "Edit labels"
            },
            id = escape(&machine.id),
            value = escape(&machine.labels.join(", ")),
        )
    } else {
        String::new()
    };
    if tags.is_empty() && edit.is_empty() {
        return String::new();
    }
    format!(r#"<div class="tags head-tags">{tags}{edit}</div>"#)
}

/// Its jobs: the numbers, and the latest, newest first.
/// Its queue (crate::queue): drawn by machine.js from the live data, next first; dragged to
/// another order, taken out with ×.
fn queue_card(machine: &Machine) -> String {
    if machine.kind == maker_connect_protocol::MachineKind::Label {
        return String::new();
    }
    // A laser's queue never starts by itself (crate::store::StartedBy).
    let laser = machine.kind == maker_connect_protocol::MachineKind::Laser;
    // HS7b: a laser takes one loaded job and has no queue; what waits here is from before.
    let (sub, empty) = if laser {
        (
            "A laser takes one loaded job, no queue. What still waits here is from before: Load one, or take it out.",
            r#"Nothing waits, and nothing new comes here: <a href="../files">Load a file</a> from Files; it shows on Control."#,
        )
    } else {
        (
            "Next first; it starts by itself when the machine is free and its bed clear. Drag your own prints to change their order; staff move any.",
            r#"Nothing waits. "Print on" a busy machine puts a file here, or drag one from Files onto this machine in the sidebar."#,
        )
    };
    card(
        &format!("Queue on {}", escape(&machine.name)),
        sub,
        &format!(
            r#"<p class="notice q-held" data-queue-held hidden><span>The last job <b data-held-why></b>: nothing starts until the bed is clear. Is it?</span><button type="button" data-bed-clear>Bed clear: go on</button></p><ol class="queue" data-queue></ol><p class="empty" data-queue-empty>{empty}</p>"#
        ),
    )
}

fn jobs_card(machine: &Machine, extra: &Extra) -> String {
    if machine.kind == maker_connect_protocol::MachineKind::Label {
        return String::new();
    }
    let s = &extra.stats;
    let ended = s.finished + s.stopped + s.failed;
    let numbers = if ended == 0 {
        r#"<p class="empty">No job has ended on it yet.</p>"#.to_owned()
    } else {
        let hours = s.seconds as f64 / 3600.0;
        let rate = format!("{:.0} %", s.finished as f64 * 100.0 / ended as f64);
        format!(
            r#"<div class="stats"><span><b>{rate}</b><small>succeeded</small></span><span><b>{ended}</b><small>jobs run</small></span>{parts}<span><b>{hours}</b><small>printing</small></span>{grams}{service}</div>"#,
            parts = if s.parts > 0 {
                format!(
                    r#"<span><b>{}</b><small>parts made</small></span>"#,
                    s.parts
                )
            } else {
                String::new()
            },
            service = machine.upkeep.as_ref().map_or(String::new(), |u| {
                let left = u.every_h - u.since_h;
                if left <= 0.0 {
                    r#"<span class="due"><b>now</b><small>service due</small></span>"#.to_owned()
                } else {
                    format!(r#"<span><b>{left:.0} h</b><small>to the next service</small></span>"#)
                }
            }),
            rate = rate,
            hours = if hours < 10.0 {
                format!("{hours:.1} h")
            } else {
                format!("{hours:.0} h")
            },
            grams = if s.grams > 0.0 {
                format!(
                    r#"<span><b>{:.0} g</b><small>filament (slicer)</small></span>"#,
                    s.grams
                )
            } else {
                String::new()
            },
        )
    };
    let rows: String = extra
        .jobs
        .iter()
        .map(|j| {
            let state = j.state.name();
            let took = match (j.started_at, j.ended_at) {
                (Some(a), Some(b)) => duration((b - a).max(0) as u64),
                _ => String::new(),
            };
            format!(
                r#"<li><span class="j-name" title="{name}">{name}</span><b class="badge {state}">{state}</b><small>{when}{by}{took}{why}</small></li>"#,
                name = escape(&j.file_name),
                by = j.by.as_deref().map(|b| format!(" · {}", escape(b))).unwrap_or_default(),
                when = crate::spoolman::iso(j.created_at).replace('T', " ").chars().take(16).collect::<String>(),
                took = if took.is_empty() { took } else { format!(" · {took}") },
                why = j.reason.as_deref().map(|r| format!(" · {}", escape(r))).unwrap_or_default(),
            )
        })
        .collect();
    card(
        "History",
        "What ran on it, newest first",
        &format!(
            r#"{numbers}<ol class="job-history">{}</ol>"#,
            if rows.is_empty() {
                r#"<li class="empty">None yet</li>"#.to_owned()
            } else {
                rows
            }
        ),
    )
}

/// What its upkeep counts, in the words of its kind (ours, UI only): hours of its jobs.
fn work_hours(kind: MachineKind) -> &'static str {
    match kind {
        MachineKind::Laser => "burning hours",
        MachineKind::Cnc | MachineKind::Cutter => "cutting hours",
        _ => "printing hours",
    }
}

/// Its upkeep: a service every so many printing hours; who manages it sets that, and says
/// when the service was done.
fn upkeep_card(machine: &Machine, manage: bool) -> String {
    let hours = work_hours(machine.kind);
    let state = match &machine.upkeep {
        None => "No service interval set.".to_owned(),
        Some(u) if u.due => format!(
            "<b>Service due</b>: {:.0} of {:.0} {hours}. It takes no work from the queue until it is done.",
            u.since_h, u.every_h
        ),
        Some(u) => format!(
            "{:.0} of {:.0} {hours} since the last service: {:.0} h to go.",
            u.since_h,
            u.every_h,
            u.every_h - u.since_h
        ),
    };
    let forms = if manage {
        format!(
            r#"<form method="post" action="{id}/upkeep" data-async data-done="Saved" data-then="reload" class="upkeep-form"><label>Service every <input name="every_h" type="number" min="1" step="1" value="{every}" placeholder="200" style="width:7em"> {hours}</label><button class="ghost">Save</button></form>
<form method="post" action="{id}/serviced" data-async data-done="Maintenance noted: the count starts again" data-then="reload" data-confirm="The service is done: start counting again?"><button>Maintenance done</button></form>"#,
            id = escape(&machine.id),
            every = machine
                .upkeep
                .as_ref()
                .map(|u| format!("{:.0}", u.every_h))
                .unwrap_or_default(),
        )
    } else {
        String::new()
    };
    card(
        "Upkeep",
        match machine.kind {
            MachineKind::Fdm => "Cleaning, lubrication, belts, nozzle: every so many hours",
            MachineKind::Laser => "Cleaning the lens, belts, air assist: every so many hours",
            MachineKind::Label => "Cleaning the print head and rollers: every so many hours",
            _ => "Cleaning and checking: every so many hours",
        },
        &format!(r#"<p>{state}</p>{forms}"#),
    )
}

/// Where the controls would be, for someone without 'operate' on this machine (ui.md, the
/// control rule and Ask for permission): one notice of what they may not do, no buttons that
/// would be refused; for who may use it, Ask for permission (a request for 'operate'), or when
/// they asked. Who may not use it asks for that first, in the notice above the tabs.
fn no_operate(machine: &Machine, extra: &Extra) -> String {
    let ask = if !extra.may_use {
        String::new()
    } else {
        match extra.asked_operate {
            Some(at) => format!(
                "<small>Asked on {}.</small>",
                crate::spoolman::iso(at)
                    .chars()
                    .take(10)
                    .collect::<String>()
            ),
            None => format!(
                r#"<form method="post" action="{}/ask" data-async data-then="reload" data-done="Asked"><input type="hidden" name="level" value="operate"><button>Ask for permission</button></form>"#,
                escape(&machine.id)
            ),
        }
    };
    format!(
        r##"<div class="notices"><div class="notice before-use"><span><b>You may not move, heat or send G-code to {}.</b> That takes 'operate' permission on it ('use' is jobs only); the space's staff give it.</span>{ask}</div></div>"##,
        escape(&machine.name)
    )
}

/// Above the tabs, when it matters: may this person use it (else: ask the staff), and the
/// PPE to have and wear before they do (crate::safety).
fn before_use(machine: &Machine, extra: &Extra) -> String {
    let id = escape(&machine.id);
    if !extra.may_use {
        let (says, button) = if machine.training {
            ("This machine needs a training.", "Ask for a training")
        } else {
            (
                "You don't have permission to use this machine.",
                "Ask for permission",
            )
        };
        let ask = match extra.asked {
            Some(at) => format!(
                "<small>Asked on {}.</small>",
                crate::spoolman::iso(at)
                    .chars()
                    .take(10)
                    .collect::<String>()
            ),
            None => format!(
                r#"<form method="post" action="{id}/ask" data-async data-then="reload" data-done="Asked"><button>{button}</button></form>"#
            ),
        };
        return format!(r#"<div class="notice before-use"><span>{says}</span>{ask}</div>"#);
    }
    if machine.ppe.is_empty() || !extra.ppe_checks {
        return String::new();
    }
    match extra.ppe_until {
        Some(until) => format!(
            r#"<div class="notice before-use ok"><span>✓ You have and wear: {items}. <small class="muted">Until <time data-local-time="{until}"></time>.</small></span></div>"#,
            items = escape(&machine.ppe.join(", ").to_lowercase()),
        ),
        None => {
            let boxes: String = machine
                .ppe
                .iter()
                .map(|item| {
                    format!(
                        r#"<label class="ppe-item"><input type="checkbox" name="have" value="{v}" required> {v}</label>"#,
                        v = escape(item)
                    )
                })
                .collect();
            format!(
                r#"<form class="notice before-use ppe" method="post" action="{id}/ppe" data-async data-then="reload" data-done="Thank you: be safe"><span><b>Before you use it:</b> I have and wear</span><span class="ppe-items">{boxes}</span><button>Confirm</button></form>"#
            )
        }
    }
}

/// The build plate on a 3D printer: which kind (smooth PEI, textured, satin…), so the queue
/// only sends it plates whose file wants that kind. Plates are swapped between printers by
/// hand; whoever does it says which is on now.
fn sheet_card(machine: &Machine) -> String {
    if machine.kind != MachineKind::Fdm {
        return String::new();
    }
    let on = machine.sheet.as_deref().unwrap_or("");
    card(
        "Build plate",
        "Swapped one? Say which is on: the queue follows it",
        &format!(
            r#"<p>{now}</p><form method="post" action="{id}/sheet" data-async data-done="Build plate noted" data-then="reload" class="upkeep-form"><input name="sheet" value="{on}" maxlength="40" list="sheet-kinds" placeholder="Smooth PEI" aria-label="The build plate on it"><datalist id="sheet-kinds"><option value="Smooth PEI"><option value="Textured PEI"><option value="Satin"><option value="Glass"><option value="Garolite"><option value="PEO"></datalist><button class="ghost">Save</button></form>"#,
            now = if on.is_empty() {
                "Not said: a file that wants a certain plate does not go to it.".to_owned()
            } else {
                format!("<b>{}</b> is on.", escape(on))
            },
            id = escape(&machine.id),
            on = escape(on),
        ),
    )
}

/// A 3D printer's filament: the spool loaded (from Materials), and loading or unloading it,
/// hot to that spool's temperature first.
fn filament_card(machine: &Machine, operate: bool) -> String {
    if machine.kind != MachineKind::Fdm || !store::takes_commands(&machine.link) {
        return String::new();
    }
    let loaded = machine.loaded.as_ref().map_or(
        r#"<p class="empty">No spool said to be loaded: <a href="../stock#board">choose one on the Materials board</a>.</p>"#.to_owned(),
        |l| {
            format!(
                r#"<p class="loaded-spool"><i class="m-swatch" style="background:{color}"></i><b>{name}</b> <small>{material}{temp} · {item}</small></p>"#,
                color = escape(l.color.as_deref().unwrap_or("transparent")),
                name = escape(&l.name),
                material = escape(l.material.as_deref().unwrap_or("")),
                temp = l.nozzle_c.map(|c| format!(" · {c:.0} °C")).unwrap_or_default(),
                item = escape(&l.item),
            )
        },
    );
    if !operate {
        return card(
            "Filament",
            "The spool on it, from Materials",
            &format!(r#"{loaded}<a class="ghost-link" href="../stock#board">Change spool</a>"#),
        );
    }
    card(
        "Filament",
        "Loaded, unloaded: it heats to the spool's temperature first",
        &format!(
            r#"{loaded}<form class="command" method="post" action="{id}/command"><button name="action" value="load-filament">Load</button><button name="action" value="unload-filament" class="ghost">Unload</button> <a class="ghost-link" href="../stock#board">Change spool</a></form><p class="note" data-command-status role="status"></p>"#,
            id = escape(&machine.id),
        ),
    )
}

pub(crate) fn overview(machine: &Machine, profile: Option<&Profile>, side: &str) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let label = machine.kind == MachineKind::Label;
    let material = t
        .and_then(|t| t.material.clone())
        .or_else(|| machine.details.as_ref().and_then(|d| d.material.clone()));
    // A label printer without a profile: the hub does not know which kind, so no cassette or roll tiles.
    let tiles = if label && profile.is_none() {
        job_tile(machine)
    } else if label {
        let media = profile.and_then(|p| profiles::media(p, machine.media.as_deref()?));
        // A LabelWriter: a roll of die-cut labels, mains power; a LabelManager: a cassette
        // of tape, and a battery.
        let die_cut = profile.is_some_and(|p| p.firmware.as_deref() == Some("dymo-lw"));
        let has_battery =
            profile.is_some_and(|p| p.battery) || t.is_some_and(|t| t.battery_percent.is_some());
        let roll = t.map(|t| {
            let empty = t
                .problem
                .as_deref()
                .is_some_and(|p| p.starts_with("no labels"));
            if empty { "Empty" } else { "Loaded" }.to_owned()
        });
        tile(
            if die_cut { "Roll" } else { "Cassette" },
            if die_cut { "roll" } else { "cassette" },
            if die_cut {
                roll
            } else {
                t.and_then(|t| t.cassette)
                    .map(|on| if on { "Loaded" } else { "Empty" }.to_owned())
            },
            "",
            "problem",
            t.and_then(|t| t.problem.clone()).unwrap_or_default(),
        ) + &tile(
            if die_cut { "Labels" } else { "Tape" },
            "none",
            media.map(|m| match m.length_mm {
                Some(length) => format!("{} × {length}", m.width_mm),
                None => m.width_mm.to_string(),
            }),
            "mm",
            "none",
            media.map_or("say which under Label".to_owned(), |m| {
                format!("{} · {}", m.name, m.id)
            }),
        ) + &(if has_battery {
            battery_tile(t.and_then(|t| t.battery_percent))
        } else {
            String::new()
        }) + &tile(
            "Job",
            "job",
            machine.job.as_ref().map(|j| j.file_name.clone()),
            "",
            "none",
            String::new(),
        )
    } else if machine.kind == MachineKind::Scale {
        // E11: one live line, as the scale says it (the page's script builds the same).
        tile(
            "Scale",
            "scale",
            t.and_then(|t| t.scale.as_ref()).map(scale_line),
            "",
            "none",
            String::new(),
        )
    } else if heats(machine) {
        // The material and the job have cards of their own (printer.html): the heaters here.
        let _ = material;
        heater_tiles(machine) + &fan_tiles(machine)
    } else {
        position_tiles(machine)
            + &tile(
                "Laser",
                "none",
                machine
                    .laser_w
                    .or(profile.and_then(|p| p.laser_w))
                    .map(|w| w.to_string()),
                "W",
                "none",
                if machine.laser_w.is_some() {
                    "optical power, as you said".to_owned()
                } else {
                    "optical power".to_owned()
                },
            )
    };
    let picture = profile
        .and_then(Profile::image)
        .or_else(|| profiles::generic_image(machine.kind))
        .unwrap_or(r#"<div class="no-picture">No picture yet</div>"#);
    // A small summary here; everything it is and told is under Settings (`machine_facts`).
    let nozzle = t
        .and_then(|t| t.identity.as_ref())
        .and_then(|i| i.nozzle_mm.clone())
        .map(|n| format!("{n} mm"))
        .or_else(|| profile.and_then(|p| p.nozzle_mm).map(|n| format!("{n} mm")));
    let facts = match profile {
        Some(p) => [
            row("Profile", Some(profile_name(machine, p))),
            row("Firmware", t.and_then(|t| t.firmware.clone())),
            row("Nozzle", nozzle),
        ]
        .concat(),
        None => "<dt>Profile</dt><dd>Not known yet: choose one under Settings</dd>".to_owned(),
    };
    let machine_card = card(
        "Machine",
        "",
        &format!(
            r#"<div class="machine-card"><div class="picture">{picture}</div><dl class="kv">{facts}</dl></div>"#
        ),
    );
    let notice = if profile.is_none() {
        no_profile_notice()
    } else {
        String::new()
    };
    // A label printer or a scale: its tiles and the machine, as before.
    if label || machine.kind == MachineKind::Scale {
        return format!(
            r#"{notice}<div class="tiles">{tiles}</div>
<div class="grid-2">{machine_card}{side}</div>"#
        );
    }
    // printer.html: This print and Live on the left; the material, what is next, the machine
    // on the right.
    let side_cards = next_card(machine);
    format!(
        r#"{notice}<div class="cols"><div class="stack">{job}<div class="card"><h3>Live</h3><div class="tiles">{tiles}</div></div></div><div class="stack">{side_cards}{machine_card}{side}</div></div>"#,
        job = job_card(machine),
    )
}

/// A printer's Now tab (printer.html "Now"): its camera beside this print; what waits in its
/// queue (kept live by machine.js) beside what ran last.
fn now(machine: &Machine, profile: Option<&Profile>, extra: &Extra) -> String {
    let notice = if profile.is_none() {
        no_profile_notice()
    } else {
        String::new()
    };
    let camera = extra.camera_now.clone().unwrap_or_else(|| {
        r##"<div class="card now-cam"><h3>Camera</h3><p class="empty">No camera on it. Cameras are linked to a machine in <a href="../space#cameras">Space → Cameras</a>.</p></div>"##.to_owned()
    });
    let queue = if machine.queue.is_empty() {
        r#"<li class="empty">Nothing waits.</li>"#.to_owned()
    } else {
        machine
            .queue
            .iter()
            .take(NOW_QUEUE)
            .enumerate()
            .map(|(i, q)| {
                format!(
                    r#"<li><span class="q-n">{}</span><span class="q-name"><b>{}</b><small>{}</small></span></li>"#,
                    i + 1,
                    escape(&q.file_name),
                    escape(q.by.as_deref().unwrap_or(""))
                )
            })
            .collect()
    };
    let recent: String = extra
        .jobs
        .iter()
        .take(NOW_RECENT)
        .map(|j| {
            let state = j.state.name();
            format!(
                r#"<li><span class="j-name" title="{name}">{name}</span><b class="badge {state}">{state}</b><small>{when}{by}</small></li>"#,
                name = escape(&j.file_name),
                when = crate::spoolman::iso(j.created_at)
                    .replace('T', " ")
                    .chars()
                    .take(16)
                    .collect::<String>(),
                by = j
                    .by
                    .as_deref()
                    .map(|b| format!(" · {}", escape(b)))
                    .unwrap_or_default(),
            )
        })
        .collect();
    format!(
        r##"{notice}<div class="now-row">{camera}{job}</div>
<div class="now-row"><div class="card"><div class="card-head"><h3>Next in its queue</h3><a href="#queue">The whole queue</a></div><ol class="now-queue" data-now-queue>{queue}</ol></div><div class="card"><div class="card-head"><h3>Recent jobs</h3><a href="#queue">All jobs</a></div><ol class="job-history">{recent}</ol></div></div>"##,
        job = if machine.kind == MachineKind::Laser {
            laser_job(machine, extra)
        } else {
            job_card(machine)
        },
        recent = if recent.is_empty() {
            r#"<li class="empty">None yet</li>"#.to_owned()
        } else {
            recent
        },
    )
}

/// A laser's job on Now (laser.html): its loaded job and the steps to burn it, above the job
/// card, which is always there: it carries the laser's Stop (a soft reset, only while a job runs)
/// and shows a job's progress live once it burns.
fn laser_job(machine: &Machine, extra: &Extra) -> String {
    format!(
        r#"<div class="stack">{}{}</div>"#,
        loaded_card(machine, extra),
        job_card(machine)
    )
}

/// How many of its queue and its last jobs the Now tab lists (the Queue tab has them all).
const NOW_QUEUE: usize = 3;
const NOW_RECENT: usize = 5;

/// What starts next here (printer.html "Next in its queue").
fn next_card(machine: &Machine) -> String {
    let next = match machine.queue.first() {
        Some(q) => format!(
            "<p>{}{}</p>",
            escape(&q.file_name),
            q.by.as_deref()
                .map_or(String::new(), |b| format!(" · {}", escape(b)))
        ),
        None => r#"<p class="muted">Nothing waits.</p>"#.to_owned(),
    };
    format!(
        r##"<div class="card"><h3>Next in its queue</h3>{next}<p><a href="#queue">All jobs on {}</a></p></div>"##,
        escape(&machine.name)
    )
}

/// A machine without a profile: said plainly at the top, with the way to choose one (owner,
/// 2026-11-17: a LabelWriter without its profile got the LabelManager's page and looked off).
pub(crate) fn no_profile_notice() -> String {
    r##"<div class="notices" style="margin: 0 0 16px"><div class="notice ask"><span><b>No profile yet.</b> Choose what this machine is under Settings: its pages are made for it (a label printer its labels, a printer its bed).</span><div class="notice-buttons"><a class="button" href="#settings">Choose a profile</a></div></div></div>"##.to_owned()
}

fn commands(machine: &Machine) -> String {
    if machine.commands.is_empty() {
        return r#"<li class="empty">None yet</li>"#.to_owned();
    }
    machine.commands.iter().map(command_item).collect()
}

/// One command in a list; the script builds the same.
fn command_item(command: &store::Command) -> String {
    let state = serde_json::to_value(command.state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    format!(
        r#"<li><span>{}</span><b class="badge {state}">{state}</b>{}</li>"#,
        escape(&command.label),
        command
            .reason
            .as_deref()
            .map(|r| format!(" <small>{}</small>", escape(r)))
            .unwrap_or_default()
    )
}

/// `refused`: the notice shown instead of the buttons without 'operate' ([`no_operate`]).
fn control(
    machine: &Machine,
    profile: Option<&Profile>,
    loaded: &str,
    operate: bool,
    refused: &str,
) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    // An axis the machine has not told: "–".
    let position = t.and_then(|t| t.position_mm);
    let z_max = profile.map_or(0.0, |p| p.build_volume_mm[2]);
    let axis = |name: &str, key: &str, i: usize| {
        format!(
            r#"<span><small>{name}</small><b data-live="{key}">{}</b><small>mm</small></span>"#,
            position
                .and_then(|p| p[i])
                .map_or("–".to_owned(), |v| format!("{v:.1}"))
        )
    };
    // Firmware that does not report the head by itself: the numbers are the last M114's.
    let read = reads_position(machine);
    let note = if read {
        r#"<p class="hint" data-position-note>Last read (M114): this firmware does not report where the head is by itself (<code>Cap:AUTOREPORT_POS:0</code>).</p>"#
    } else {
        ""
    };
    let xyz = axis("X", "pos_x", 0)
        + &axis("Y", "pos_y", 1)
        + &if z_max > 0.0 {
            axis("Z", "pos_z", 2)
        } else {
            String::new()
        };
    // Z beside the bed, as a bar that fills to the head's height.
    let gauge = if z_max > 0.0 {
        let filled = position
            .and_then(|p| p[2])
            .map_or(0.0, |z| (z as f64 / z_max * 100.0).clamp(0.0, 100.0));
        format!(
            r#"<div class="zgauge" data-z-max="{z_max}" title="Z, of {z_max} mm"><span>{z_max}</span><div class="ztrack"><i data-z-fill style="height: {filled:.1}%"></i></div><span>0</span></div>"#
        )
    } else {
        String::new()
    };
    let bed = card(
        if heats(machine) { "Bed" } else { "Work area" },
        "The head where the machine says it is, live",
        &profile
            // a profile from its firmware knows no bed (M115 does not say it)
            .filter(|p| p.build_volume_mm[0] > 0.0)
            .map_or(
                r#"<p class="empty">Choose a profile under Settings to see the bed.</p>"#
                    .to_owned(),
                |p| {
                    format!(
                        r#"<div class="xyz">{xyz}</div>{note}<div class="bed-row">{}{gauge}</div>"#,
                        profiles::bed_svg(p)
                    )
                },
            ),
    );
    // Without 'operate': where the head is, and one notice; no button that would be refused.
    if !operate && store::takes_commands(&machine.link) {
        return format!(
            r#"{refused}<div class="grid-control">{bed}<div class="stack">{loaded}</div></div>"#
        );
    }
    if !store::takes_commands(&machine.link) {
        return format!(
            r#"<div class="grid-control">{bed}{}</div>"#,
            card(
                "Control",
                "",
                r#"<p class="empty">Commands reach printers on built-in Prusa Connect and virtual machines for now; agents follow.</p>"#
            )
        );
    }
    let button = |action: &str, text: &str| {
        format!(r#"<button name="action" value="{action}">{text}</button>"#)
    };
    let fdm = heats(machine);
    let mut home = button("home", "Home all");
    if read {
        home += &button("read-position", "Read position");
    }
    if fdm {
        return printer_controls(
            machine, profile, t, loaded, &xyz, note, &gauge, &home, &button,
        );
    }
    // laser.html: one card in two columns, as a printer's; the loaded job below.
    let area = profile
        // a profile from its firmware knows no bed (M115 does not say it)
        .filter(|p| p.build_volume_mm[0] > 0.0)
        .map_or(
            r#"<p class="empty">Choose a profile under Machine to see the work area.</p>"#
                .to_owned(),
            |p| {
                format!(
                    r#"<div class="bed-row">{}{gauge}</div>"#,
                    profiles::bed_svg(p)
                )
            },
        );
    let left = format!(
        r#"<h4>Coordinates</h4><div class="xyz">{xyz}</div>{area}{}"#,
        jog_pad()
    );
    let right = format!(
        r#"<h4>Origin</h4><div class="buttons">{}{}</div><p class="hint">A job starts from its origin. Move the head to the corner of what you work on, then set it; the machine forgets it when it restarts.</p><h4>Machine</h4><div class="buttons">{}</div><p class="hint">Unlock after an alarm ($X). A laser has no console: nothing but Start burn switches the laser on.</p>"#,
        button("set-origin", "Set origin here"),
        button("go-origin", "Go to origin"),
        button("unlock", "Unlock"),
    );
    format!(
        r#"{}<div class="grid-2 control-below">{loaded}</div>"#,
        card(
            "Controls",
            "Sent one at a time; History lists how each went",
            &format!(
                r#"<form class="command" method="post" action="{}/command"><div class="ctl-cols"><div>{left}</div><div>{right}</div></div></form><p class="note" data-command-status role="status"></p>"#,
                escape(&machine.id)
            )
        ),
    )
}

/// A printer's Controls (Prusa Connect's layout, our own build): one card, moving, the
/// extruder and the fan on the left; temperatures, speed and flow on the right. One form, so
/// each button sends its own action as before. Below it the spool; what was sent is under History.
#[allow(clippy::too_many_arguments)]
fn printer_controls(
    machine: &Machine,
    profile: Option<&Profile>,
    t: Option<&maker_connect_protocol::Telemetry>,
    loaded: &str,
    xyz: &str,
    note: &str,
    gauge: &str,
    home: &str,
    button: &dyn Fn(&str, &str) -> String,
) -> String {
    let bed = profile
        // a profile from its firmware knows no bed (M115 does not say it)
        .filter(|p| p.build_volume_mm[0] > 0.0)
        .map_or(
            r#"<p class="empty">Choose a profile under Machine to see the bed.</p>"#.to_owned(),
            |p| {
                format!(
                    r#"<div class="bed-row">{}{gauge}</div>"#,
                    profiles::bed_svg(p)
                )
            },
        );
    // Prusa's MK3 family: no fan control, as PrusaLink has none.
    let fan = if crate::prusa_mk3_machine(machine) {
        String::new()
    } else {
        format!(
            r#"<h4>Print fan</h4><div class="buttons">{}{}</div>"#,
            button("fan-on", "On"),
            button("fan-off", "Off")
        )
    };
    let pad = printer_pad();
    // A printer on Prusa Connect: the server's Mesh bed leveling beside Home (crate::probe_gcode);
    // others measure from their bed level section, where the mesh comes back.
    let level = if machine.link == "prusa-connect" {
        button(
            "probe-bed",
            crate::devices::legacy_gcode::MESH_BED_LEVELING.0,
        )
    } else {
        String::new()
    };
    let left = format!(
        r#"<h4>Coordinates</h4><div class="xyz">{xyz}</div>{note}{bed}{pad}<div class="buttons">{home}{level}{}</div><h4>Extruder</h4><div class="buttons">{}{}</div><p class="hint">10 mm at a time; the nozzle must be hot (the printer refuses a cold extrusion).</p>{fan}"#,
        button("motors-off", "Motors off"),
        button("extrude", "Extrude 10 mm"),
        button("retract", "Retract 10 mm"),
    );
    let right = format!(
        r#"<h4>Temperatures</h4><div class="buttons">{}{}{}</div>{}{}"#,
        button("preheat-pla", "Preheat PLA"),
        button("preheat-petg", "Preheat PETG"),
        button("cooldown", "Cool down"),
        heat_inputs(t, profile),
        factor_inputs(t),
    );
    format!(
        r#"{}<div class="grid-2 control-below">{loaded}</div>"#,
        card(
            "Controls",
            "Sent one at a time; History lists how each went",
            &format!(
                r#"<form class="command" method="post" action="{}/command"><div class="ctl-cols"><div>{left}</div><div>{right}</div></div></form><p class="note" data-command-status role="status"></p>"#,
                escape(&machine.id)
            )
        ),
    )
}

/// A printer's arrow pad (Prusa Connect's Controls, our own build): X and Y in a cross, Z up
/// and down beside it, by the step chosen (0.1, 1 or 10 mm); the arrow keys move X and Y, Page
/// Up and Page Down Z (machine.js). Sent as `jog:<x>:<y>` and `jog:0:0:<z>` (crate::commands).
fn printer_pad() -> String {
    let arrow = |action: &str, text: &str, what: &str| {
        format!(
            r#"<button name="action" value="{action}" title="Move {what}" aria-label="Move {what}">{text}</button>"#
        )
    };
    let steps: String = [(0.1, "0.1"), (1.0, "1"), (10.0, "10")]
        .iter()
        .map(|(v, t)| {
            format!(
                r#"<label><input type="radio" name="step" value="{v}"{}><span>{t}</span></label>"#,
                if *v == 1.0 { " checked" } else { "" }
            )
        })
        .collect();
    format!(
        r#"<div class="jogpad printer-pad" tabindex="0" data-jogpad aria-label="Move the head: the arrow keys move X and Y, Page Up and Page Down Z, by the step">
  <div class="jog-steps" role="radiogroup" aria-label="Step in mm"><small>Step</small>{steps}<small>mm</small></div>
  <div class="pad-row"><div class="jog-grid"><span></span>{}<span></span>{}<span class="pad-xy">X/Y</span>{}<span></span>{}<span></span></div>
  <div class="jog-grid jog-z">{}<span class="pad-xy">Z</span>{}</div></div>
</div>"#,
        arrow("jog:0:1", "↑", "back (Y+)"),
        arrow("jog:-1:0", "←", "left (X−)"),
        arrow("jog:1:0", "→", "right (X+)"),
        arrow("jog:0:-1", "↓", "to the front (Y−)"),
        arrow("jog:0:0:1", "↑", "up (Z+)"),
        arrow("jog:0:0:-1", "↓", "down (Z−)"),
    )
}

/// The speed and flow factors: a field each with its button, filled with what the machine
/// says (or 100 %).
fn factor_inputs(t: Option<&maker_connect_protocol::Telemetry>) -> String {
    let field = |name: &str, label: &str, value: Option<f32>, min: u32, max: u32| {
        format!(
            r#"<label class="heat-set"><span>{label}</span><input type="number" name="{name}" value="{:.0}" min="{min}" max="{max}" step="1" inputmode="numeric" aria-label="{label} in percent"><small>%</small><button name="action" value="set-{name}" class="ghost">Set</button></label>"#,
            value.unwrap_or(100.0)
        )
    };
    format!(
        r#"<h4>Speed and flow</h4><div class="heat-sets">{}{}</div><p class="hint">100 % is as the file says. Speed 10 to 300 %, flow 50 to 150 %; they hold until the printer restarts.</p>"#,
        field("speed", "Speed", t.and_then(|t| t.speed_percent), 10, 300),
        field("flow", "Flow", t.and_then(|t| t.flow_percent), 50, 150),
    )
}

/// Own temperatures: a field each for the nozzle and the bed, up to the profile's maximum,
/// filled with what it is heating to (or PLA's), and a button that sets it (0 is off).
fn heat_inputs(t: Option<&maker_connect_protocol::Telemetry>, profile: Option<&Profile>) -> String {
    let field = |name: &str,
                 label: &str,
                 target: Option<f32>,
                 fallback: f32,
                 max: Option<f64>,
                 default_max: f64| {
        let value = target.filter(|v| *v > 0.0).unwrap_or(fallback);
        let max = max.unwrap_or(default_max);
        format!(
            r#"<label class="heat-set"><span>{label}</span><input type="number" name="{name}" value="{value:.0}" min="0" max="{max}" step="1" inputmode="numeric" aria-label="{label} temperature"><small>°C</small><button name="action" value="set-{name}" class="ghost">Set</button></label>"#
        )
    };
    format!(
        r#"<div class="heat-sets">{}{}</div><p class="hint">Your own temperature; 0 switches it off. Up to {} / {} °C on this machine.</p>"#,
        field(
            "nozzle",
            "Nozzle",
            t.and_then(|t| t.nozzle_target_c),
            215.0,
            profile.and_then(|p| p.max_nozzle_c),
            260.0
        ),
        field(
            "bed",
            "Bed",
            t.and_then(|t| t.bed_target_c),
            60.0,
            profile.and_then(|p| p.max_bed_c),
            100.0
        ),
        profile.and_then(|p| p.max_nozzle_c).unwrap_or(260.0),
        profile.and_then(|p| p.max_bed_c).unwrap_or(100.0),
    )
}

/// The terminal: the G-code sent from here and what the machine answered, and a line to
/// type more. The script fills it from the machine's commands.
fn terminal(machine: &Machine) -> String {
    if !store::takes_commands(&machine.link) {
        return card(
            "Terminal",
            "",
            r#"<p class="empty">This machine takes no commands yet.</p>"#,
        );
    }
    let sub = if machine.link == "prusa-connect" {
        "G-code you send runs on the printer; over Prusa Connect its answers do not come back, only whether it ran"
    } else {
        "G-code you send, and what the machine answers"
    };
    let quick = ["M115", "M105", "M114", "G28"]
        .map(|g| {
            format!(r#"<button type="button" class="ghost" data-gcode-quick="{g}">{g}</button>"#)
        })
        .concat();
    card(
        "Terminal",
        sub,
        &format!(
            r#"<pre class="terminal" data-terminal aria-live="polite"></pre>
<form class="gcode" data-gcode action="{}/command">
  <input name="gcode" placeholder="G-code, e.g. M115" autocomplete="off" spellcheck="false" aria-label="G-code to send">
  <button>Send</button>
</form>
<div class="buttons quick">{quick}</div>
<p class="note" data-gcode-status role="status"></p>"#,
            escape(&machine.id)
        ),
    )
}

/// A printer's fans, as tiles (printer.html "Live").
fn fan_tiles(machine: &Machine) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let rpm = |v: Option<u32>| v.map(|v| v.to_string());
    // Where a printer tells its fans only while it prints, that is said while none is told.
    let fan_note = |rpm: Option<u32>| {
        if rpm.is_some() {
            String::new()
        } else {
            "reported while printing".to_owned()
        }
    };
    tile(
        "Print fan",
        "print_fan_n",
        rpm(t.and_then(|t| t.print_fan_rpm)),
        "rpm",
        "none",
        fan_note(t.and_then(|t| t.print_fan_rpm)),
    ) + &tile(
        "Hotend fan",
        "hotend_fan_n",
        rpm(t.and_then(|t| t.hotend_fan_rpm)),
        "rpm",
        "none",
        fan_note(t.and_then(|t| t.hotend_fan_rpm)),
    )
}

fn telemetry_tab(machine: &Machine) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    // What was sent from the hub, and how it went (Prusa Connect's History tab).
    let sent = if store::takes_commands(&machine.link) {
        card(
            "Commands",
            "Sent from here, newest last; each with how it went",
            &format!(
                r#"<ol class="commands" data-commands>{}</ol>"#,
                commands(machine)
            ),
        )
    } else {
        String::new()
    };
    let percent = |v: Option<f32>| v.map(|v| format!("{v} %"));
    let motion = [
        live_row(
            "Position",
            "position",
            t.and_then(|t| t.position_mm).map(|p| {
                let axis = |v: Option<f32>| v.map_or("–".to_owned(), |v| format!("{v:.1}"));
                format!("{} · {} · {} mm", axis(p[0]), axis(p[1]), axis(p[2]))
            }),
        ),
        live_row("Speed", "speed", percent(t.and_then(|t| t.speed_percent))),
    ]
    .concat();
    if !heats(machine) {
        return format!(
            r#"<div class="tiles">{}</div><div class="grid-2">{}</div>{sent}"#,
            position_tiles(machine),
            card("Motion", "", &format!(r#"<dl class="kv">{motion}</dl>"#))
        );
    }
    let printing = [
        live_row("Flow", "flow", percent(t.and_then(|t| t.flow_percent))),
        live_row("Material", "material", t.and_then(|t| t.material.clone())),
        live_row("Sheet", "sheet", t.and_then(|t| t.sheet.clone())),
        live_row("Mode", "print_mode", t.and_then(|t| t.print_mode.clone())),
        live_row(
            "SD card",
            "sd_card",
            t.and_then(|t| t.sd_card).map(|i| {
                if i {
                    "in".to_owned()
                } else {
                    "none".to_owned()
                }
            }),
        ),
        live_row("MMU", "mmu", mmu_text(t)),
        live_row("Doing", "doing", t.and_then(|t| t.doing.clone())),
    ]
    .concat();
    let chart = r#"<div class="card chart-card">
  <div class="card-head">
    <div><h3>Temperatures</h3><p class="card-sub">The last 2 hours, live · targets dashed</p></div>
    <button type="button" class="ghost" data-table-toggle aria-pressed="false">Table</button>
  </div>
  <div class="legend"><span><i class="key key-nozzle"></i>Nozzle</span><span><i class="key key-bed"></i>Bed</span><span><i class="key key-target"></i>Target</span></div>
  <div class="plot">
    <svg id="chart" role="img" aria-label="Nozzle and bed temperature over the last 2 hours"></svg>
    <div id="tip" class="tip" hidden></div>
  </div>
  <div class="table-wrap" data-table hidden></div>
</div>
<div class="card chart-card">
  <div class="card-head">
    <div><h3>Fans</h3><p class="card-sub">The last 2 hours, live · where the machine tells them</p></div>
  </div>
  <div class="legend"><span><i class="key key-print-fan"></i>Print fan</span><span><i class="key key-hotend-fan"></i>Hotend fan</span></div>
  <div class="plot">
    <svg id="fanchart" role="img" aria-label="Print fan and hotend fan speed over the last 2 hours"></svg>
    <div id="fantip" class="tip" hidden></div>
  </div>
</div>"#;
    format!(
        // The heaters' and fans' tiles are in Overview's Live card.
        r#"{chart}
<div class="grid-2">{}{}</div>{sent}"#,
        card("Motion", "", &format!(r#"<dl class="kv">{motion}</dl>"#)),
        card(
            "Printing",
            "",
            &format!(r#"<dl class="kv">{printing}</dl>"#)
        ),
    )
}

/// Gigabytes, as a USB stick's box says them (1000³).
fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// A GRBL machine's own settings (`$$`), and what in them matters before a job.
fn controller(machine: &Machine) -> String {
    let Some(grbl) = machine.telemetry.as_ref().and_then(|t| t.grbl.as_ref()) else {
        return String::new();
    };
    let profile = machine.profile.as_deref().and_then(profiles::find);
    let mut notes = Vec::new();
    if grbl.laser_mode == Some(false) {
        notes.push("Laser mode is off ($32=0): the beam stays on where the head stops. Jobs are refused until it is on ($32=1).".to_owned());
    }
    if let (Some([x, y, _]), Some(p)) = (grbl.travel_mm, profile) {
        let [w, d, _] = p.build_volume_mm;
        if (x - w).abs() > 1.0 || (y - d).abs() > 1.0 {
            notes.push(format!(
                "Its travel ({x:.0} × {y:.0} mm) differs from the profile's work area ({w:.0} × {d:.0} mm): jobs are checked against its travel."
            ));
        }
    }
    let rows = [
        row(
            "Laser mode ($32)",
            grbl.laser_mode
                .map(|on| if on { "on" } else { "off" }.to_owned()),
        ),
        row(
            "Full power ($30)",
            grbl.max_power.map(|s| format!("S{s:.0}")),
        ),
        row(
            "Travel ($130–$132)",
            grbl.travel_mm
                .map(|[x, y, z]| format!("{x:.0} × {y:.0} × {z:.0} mm")),
        ),
    ]
    .concat();
    let notes: String = notes
        .iter()
        .map(|n| format!(r#"<p class="note">{}</p>"#, escape(n)))
        .collect();
    card(
        "Controller",
        "Its GRBL settings, as it reported them ($$)",
        &format!("<dl>{rows}</dl>{notes}"),
    )
}

/// How a device reaches the hub, the same card for every kind: the way (its own firmware,
/// Moonraker, simulated), the computer it runs on as it tells, the address its reports come
/// from, and when the last came.
/// Whether a device on the Prusa Connect protocol is one of the agent's (a label printer, a
/// scale, a Marlin printer), not a Prusa's own firmware.
/// A Marlin printer on the agent whose M115 says it does not report the head by itself
/// (`Cap:AUTOREPORT_POS:0`, Marlin's `AUTO_REPORT_POSITION` left out): X, Y and Z are what its
/// last M114 answered, and Controls offers to read them (ui.md "Controls").
pub(crate) fn reads_position(machine: &Machine) -> bool {
    crate::prusa_connect::marlin_agent(machine)
        && machine.details.as_ref().is_some_and(|d| {
            d.firmware_capabilities
                .as_ref()
                .is_some_and(|caps| caps.get("AUTOREPORT_POS").is_some())
                && !d.can("AUTOREPORT_POS")
        })
}

fn on_agent(machine: &Machine) -> bool {
    machine.link == "prusa-connect"
        && (machine.kind != MachineKind::Fdm || crate::prusa_connect::marlin_agent(machine))
}

fn connection(machine: &Machine) -> String {
    let way = match machine.link.as_str() {
        "prusa-connect" if on_agent(machine) => "Maker Connect agent (Prusa Connect protocol)",
        "prusa-connect" => "Its own firmware (Prusa Connect)",
        "virtual" => "Virtual: simulated in the hub",
        "simplyprint" => "Moonraker, by itself (its SimplyPrint component)",
        other => other,
    };
    let agent = machine.agent.as_ref();
    let system = agent.map(|a| {
        let os = match a.os.as_str() {
            "windows" => "Windows",
            "macos" => "macOS",
            "linux" => "Linux",
            other => other,
        };
        format!("{os} ({})", a.arch)
    });
    let last = match machine.seen_seconds_ago {
        Some(_) if machine.online => "Now".to_owned(),
        Some(s) => ago(s),
        None => "Never".to_owned(),
    };
    // An agent on the Prusa Connect protocol says its release in every request
    // (`User-Agent-Version`), kept as the firmware: "Maker Connect agent v0.3.0-v2ui.9".
    let agent_release = machine
        .telemetry
        .as_ref()
        .and_then(|t| t.firmware.as_deref())
        .and_then(|f| f.strip_prefix("Maker Connect agent "))
        .map(str::to_owned);
    let rows = [
        row("Connected by", Some(way.to_owned())),
        row(
            "State",
            Some(
                if machine.online {
                    "Connected"
                } else {
                    "Not connected"
                }
                .to_owned(),
            ),
        ),
        row(
            "Agent version",
            agent
                .map(|a| match (&a.build, &a.built) {
                    (Some(build), Some(built)) => {
                        format!("{} · build {build}, {built}", a.version)
                    }
                    (Some(build), None) => format!("{} · build {build}", a.version),
                    _ => format!("{} (an agent from before builds were named)", a.version),
                })
                .or(agent_release),
        ),
        // The SDK's `sn`, which the agent makes for a device that has none to say (wire.md):
        // the hub's, to tell devices apart; not the machine's (a Marlin's own is M115's `UUID`).
        row(
            "Serial (made by the agent)",
            machine
                .details
                .as_ref()
                .and_then(|d| d.serial.clone())
                .filter(|_| on_agent(machine)),
        ),
        row("Computer", agent.and_then(|a| a.hostname.clone())),
        row("System", system),
        row("Its address", agent.and_then(|a| a.address.clone())),
        row("Reports from", machine.peer.clone()),
        row("Last report", Some(last)),
    ]
    .concat();
    // A printer on Klipper: the lines for its moonraker.conf (the address filled in by the
    // page's script, from where it was opened).
    let moonraker = machine
        .moonraker_token
        .as_deref()
        .map_or(String::new(), |token| {
            format!(
                r#"<h4>On the printer</h4><p class="hint">Add this to <code>moonraker.conf</code> on the printer's computer (Mainsail or Fluidd: Machine, then moonraker.conf), save, and restart Moonraker. It then connects here by itself.</p><pre class="conf" data-moonraker-conf data-token="{token}">[simplyprint]
url: ws://…/simplyprint/{token}</pre><p class="hint" data-moonraker-local hidden>This page was opened as <b>localhost</b>: the printer cannot reach that. Open it by this computer's address on the network, or put that address in the line yourself.</p><p class="hint">It takes the place of SimplyPrint's own hub on that printer: a printer has one.</p>"#,
                token = escape(token)
            )
        });
    card(
        "Connection",
        "How it reaches the hub",
        &format!("<dl>{rows}</dl>{moonraker}"),
    )
}

pub(crate) fn settings(
    machine: &Machine,
    move_to: &[(String, String)],
    ppe_checks: bool,
) -> String {
    let details = machine.details.clone().unwrap_or_default();
    // The profile: chosen from all the machines known, by searching (the shell's picker).
    let chosen = machine
        .profile
        .as_deref()
        .filter(|_| machine.profile_chosen)
        .unwrap_or("");
    let id = escape(&machine.id);
    let name = escape(&machine.name);
    // The profile now, plainly: its drawing, name, size, and whether chosen or recognised.
    let found = machine.profile.as_deref().and_then(profiles::find);
    let told = found.is_none().then(|| told_profile(machine)).flatten();
    let current = match found.or(told.as_ref()) {
        Some(p) => format!(
            r#"<div class="current-profile"><div class="current-picture">{picture}</div><div><b>{brand} {model}</b><small>{size}{firmware}</small><span class="profile-how">{how}</span></div></div>"#,
            picture = p.image().unwrap_or(""),
            brand = escape(&p.brand),
            model = escape(&p.model),
            size = if p.kind == MachineKind::Label || p.build_volume_mm[0] > 0.0 {
                escape(&p.size())
            } else {
                "Its bed size is not known: fill it in below, or choose a profile".to_owned()
            },
            firmware = p
                .firmware_version
                .as_deref()
                .map(|v| {
                    // "marlin" as "Marlin".
                    let firmware = p.firmware.as_deref().unwrap_or("");
                    let mut name = firmware.chars();
                    let named = name
                        .next()
                        .map(|c| c.to_uppercase().chain(name).collect::<String>())
                        .unwrap_or_default();
                    format!(" · {} {}", escape(&named), escape(v))
                })
                .unwrap_or_default(),
            how = if found.is_none() && crate::prusa_connect::marlin_agent(machine) {
                "From its firmware (M115)"
            } else if found.is_none() {
                "From its controller"
            } else if machine.profile_chosen {
                "Chosen here"
            } else {
                "Recognised from what it says"
            },
        ),
        None => r#"<p class="empty">Not known yet: it is recognised when it says what it is, or choose it below.</p>"#.to_owned(),
    };
    let chosen = escape(chosen);
    let auto = escape(
        machine
            .profile
            .as_deref()
            .filter(|_| !machine.profile_chosen)
            .unwrap_or(""),
    );
    // Out of use: why, and back; or a reason to take it out.
    // The example reason fits the kind of machine (ours, UI only).
    let why_example = match machine.kind {
        MachineKind::Fdm => "Why: nozzle clogged, waiting for a part",
        MachineKind::Laser => "Why: lens dirty, waiting for a part",
        MachineKind::Cnc => "Why: spindle bearing worn, waiting for a part",
        MachineKind::Label => "Why: cutter stuck, waiting for a part",
        _ => "Why: broken, waiting for a part",
    };
    let in_use = match &machine.out_of_use {
        Some(o) => format!(
            r#"<p class="hint">Out of use{}{}.</p><form method="post" action="{id}/out-of-use" data-async data-done="Back in use" data-then="reload"><input type="hidden" name="action" value="back"><button>Back in use</button></form>"#,
            if o.reason.is_empty() {
                String::new()
            } else {
                format!(": {}", escape(&o.reason))
            },
            if o.since > 0 {
                format!(", {}", ago(store::now() - o.since))
            } else {
                String::new()
            }
        ),
        None => format!(
            r#"<form method="post" action="{id}/out-of-use" data-async data-done="Out of use" data-then="reload"><input type="hidden" name="action" value="out"><input name="reason" maxlength="200" placeholder="{why_example}" aria-label="Why it is out of use"><button class="ghost">Take out of use</button></form><p class="hint">A machine out of use takes no jobs; commands still reach it, to mend it.</p>"#
        ),
    };
    // A laser's module: the controller knows S1000 is full power, not how many watts.
    let laser = if machine.kind == MachineKind::Laser {
        let profile_w = machine
            .profile
            .as_deref()
            .and_then(profiles::find)
            .and_then(|p| p.laser_w);
        format!(
            r#"<h4>Laser module</h4><form method="post" action="{id}/laser" data-async data-done="Saved" data-then="reload"><input name="watts" inputmode="decimal" value="{}" placeholder="{}" aria-label="Optical power in watts" style="width:7em"><small>W</small><button class="ghost">Save</button></form><p class="hint">Its optical power, as on the module (a 10 W LU2-10A). The controller does not know it{}.</p>"#,
            machine.laser_w.map_or(String::new(), |w| w.to_string()),
            profile_w.map_or("watts".to_owned(), |w| w.to_string()),
            profile_w.map_or(String::new(), |w| format!("; empty is the profile's {w} W"))
        )
    } else {
        String::new()
    };
    // To another space of whoever manages it there too: a makerspace's other workshop.
    // Safety: what someone must have and wear to use it; ticked off before each use.
    let safety = {
        let laser = machine.kind == maker_connect_protocol::MachineKind::Laser;
        let boxes: String = crate::safety::CATALOGUE
            .iter()
            // Laser glasses are offered on a laser only (kept where someone chose them).
            .filter(|item| {
                laser
                    || **item != "Laser safety glasses"
                    || machine.ppe.iter().any(|p| p.eq_ignore_ascii_case(item))
            })
            .map(|item| {
                format!(
                    r#"<label class="ppe-item"><input type="checkbox" name="item" value="{v}"{c}> {v}</label>"#,
                    v = escape(item),
                    c = if machine.ppe.iter().any(|p| p.eq_ignore_ascii_case(item)) {
                        " checked"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        let others: Vec<&str> = machine
            .ppe
            .iter()
            .filter(|p| {
                !crate::safety::CATALOGUE
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(p))
            })
            .map(String::as_str)
            .collect();
        format!(
            r#"<h4>Safety</h4>
<p class="muted">What someone must have and wear to use it. Before using it, they tick off each item; that holds for 12 hours. Stopping, pausing and switching off never wait for it.{off}</p>
<form method="post" action="{id}/safety" data-async data-done="Saved" data-then="reload" class="ppe-setting"><span class="ppe-items">{boxes}</span><input name="other" value="{others}" maxlength="120" placeholder="Something else (comma separated)" aria-label="Other PPE"><button class="ghost">Save</button></form>
"#,
            others = escape(&others.join(", ")),
            off = if ppe_checks {
                ""
            } else {
                r#" <b>The space does not ask for it now</b>: switch it on under <a href="../space#general">Space → General</a>."#
            },
        )
    };
    let move_section = if move_to.is_empty() {
        String::new()
    } else {
        format!(
            r#"<h4>Move to another space</h4>
<p class="muted">Its name, labels, build plate and service interval go along, and its agent keeps working. Its jobs, the permissions on it and the spool said to be loaded stay here; in the new space it takes no work from the queue until someone says so. Not while it has work under way.</p>
<form method="post" action="{id}/move" data-async data-done="Moved" data-then="../" data-confirm="Move this machine to the other space? You may no longer see it here." class="switch"><select name="space" aria-label="To which space" required>{options}</select><button class="ghost">Move</button></form>
"#,
            options = move_to
                .iter()
                .map(|(uid, label)| format!(
                    r#"<option value="{}">{}</option>"#,
                    escape(uid),
                    escape(label)
                ))
                .collect::<String>(),
        )
    };
    // A profile from its firmware (M115 does not say the bed): its size, as the space says it.
    let bed = if found.is_none() && told.is_some() && crate::prusa_connect::marlin_agent(machine) {
        let [x, y, z] = machine
            .bed_mm
            .map_or([String::new(), String::new(), String::new()], |b| {
                b.map(|v| v.to_string())
            });
        format!(
            r#"<form method="post" action="{id}/bed" data-async data-done="Saved" data-then="reload" class="bed-size"><label>X <input name="x" inputmode="decimal" value="{x}" placeholder="220" aria-label="Bed width X in mm" style="width:5em"></label> <label>Y <input name="y" inputmode="decimal" value="{y}" placeholder="220" aria-label="Bed depth Y in mm" style="width:5em"></label> <label>Z <input name="z" inputmode="decimal" value="{z}" placeholder="250" aria-label="Height Z in mm" style="width:5em"></label> <small>mm</small> <button class="ghost">Save</button></form><p class="hint">Its bed size: its firmware does not say it. X0 Y0 is the front left corner.</p>"#
        )
    } else {
        String::new()
    };
    let forms = format!(
        r#"<h4>Name</h4>
<form method="post" action="{id}/name" data-async data-done="Renamed" data-rename><input name="name" value="{name}" aria-label="Name" maxlength="60" required><button>Rename</button></form>
<h4>Profile</h4>
{current}
{bed}<details class="change-profile"><summary class="ghost-button">Change profile</summary>
<div class="picker in-card" data-picker data-action="{id}/profile" data-selected="{chosen}" data-auto="{auto}" data-kind="{own_kind}"></div>
</details>
{laser}<h4>In use</h4>
{in_use}
{safety}{move_section}<h4>Remove</h4>
<form method="post" action="{id}/remove" data-async data-done="Machine removed" data-confirm="Remove this machine from the space?" data-then="../"><button class="danger">Remove machine</button></form>"#,
        // a machine's profile is one of its own kind (owner, 2026-11-17: a laser was offered every printer)
        own_kind = serde_json::to_value(machine.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
    );
    let about = machine_facts(machine, found.or(told.as_ref()));
    let storage: String = details
        .storages
        .iter()
        .map(|s| {
            let free = s
                .free_bytes
                .map_or("–".to_owned(), |b| format!("{} free", gigabytes(b)));
            let read_only = if s.read_only { ", read only" } else { "" };
            row(
                &format!("{} ({})", escape(&s.kind), escape(&s.path)),
                Some(format!("{free}{read_only}")),
            )
        })
        .collect();
    let network = details.network.unwrap_or_default();
    let network = [
        row("Hostname", network.hostname),
        row("LAN address", network.lan_ipv4),
        row("LAN MAC", network.lan_mac),
        row("Wi-Fi address", network.wifi_ipv4),
        row("Wi-Fi MAC", network.wifi_mac),
        row("Wi-Fi network", network.wifi_ssid),
    ]
    .concat();
    let mut info = connection(machine);
    info += &controller(machine);
    if !storage.is_empty() {
        info += &card("Storage", "", &format!("<dl>{storage}</dl>"));
    }
    if !network.is_empty() {
        info += &card("Network", "", &format!("<dl>{network}</dl>"));
    }
    // Details first (Prusa Connect's Printer tab, our own build): what it is beside what it told.
    let (spec, told) = about;
    let column = |title: &str, rows: &str| {
        format!(
            r#"<div><h4>{title}</h4>{}</div>"#,
            if rows.is_empty() {
                r#"<p class="empty">Not known yet.</p>"#.to_owned()
            } else {
                format!(r#"<dl class="kv">{rows}</dl>"#)
            }
        )
    };
    let details = card(
        "Details",
        "Its profile, and what it told about itself",
        &format!(
            r#"<div class="ctl-cols">{}{}</div>"#,
            column("Specification", &spec),
            column("This machine", &told)
        ),
    );
    format!(
        r#"{details}<div class="grid-settings">{}<div class="stack">{info}</div></div>"#,
        card("Settings", "", &forms)
    )
}

/// The jog pad of a laser, CNC or plotter (printers keep their buttons): eight directions and
/// Home in the middle, a step and a speed to choose; the arrow keys move it too.
fn jog_pad() -> String {
    // (x, y, arrow) in the pad's order, row by row from the back.
    const PAD: [(i8, i8, &str); 9] = [
        (-1, 1, "↖"),
        (0, 1, "↑"),
        (1, 1, "↗"),
        (-1, 0, "←"),
        (0, 0, "⌂"),
        (1, 0, "→"),
        (-1, -1, "↙"),
        (0, -1, "↓"),
        (1, -1, "↘"),
    ];
    let pad: String = PAD
        .iter()
        .map(|&(x, y, arrow)| {
            if (x, y) == (0, 0) {
                format!(r#"<button name="action" value="home" class="jog-home" title="Home">{arrow}</button>"#)
            } else {
                let what = match (x, y) {
                    (0, 1) => "back (Y+)",
                    (0, -1) => "to the front (Y−)",
                    (1, 0) => "right (X+)",
                    (-1, 0) => "left (X−)",
                    _ => "diagonally",
                };
                format!(
                    r#"<button name="action" value="jog:{x}:{y}" title="Move {what}" aria-label="Move {what}">{arrow}</button>"#
                )
            }
        })
        .collect();
    let steps: String = [(0.1, "0.1"), (1.0, "1"), (10.0, "10"), (50.0, "50")]
        .iter()
        .map(|(v, t)| {
            format!(
                r#"<label><input type="radio" name="step" value="{v}"{}><span>{t}</span></label>"#,
                if *v == 10.0 { " checked" } else { "" }
            )
        })
        .collect();
    format!(
        r#"<div class="jogpad" tabindex="0" data-jogpad aria-label="Move the head: arrow keys move by the step">
  <div class="jog-grid">{pad}</div>
  <div class="jog-settings"><div class="jog-steps" role="radiogroup" aria-label="Step in mm">{steps}<small>mm</small></div>
    <label class="jog-speed">Speed <select name="feed"><option value="600">slow</option><option value="1500">medium</option><option value="3000" selected>normal</option><option value="6000">fast</option></select></label></div>
</div><p class="hint">Click the pad, then the arrow keys move it too. Home first. The laser stays off while it moves.</p>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heater_says_how_it_is_doing() {
        assert_eq!(heat_note(Some(150.0), Some(215.0)), "▲ heating to 215 °C");
        assert_eq!(heat_note(Some(214.0), Some(215.0)), "target 215 °C");
        assert_eq!(heat_note(Some(120.0), Some(0.0)), "target off · ▼ cooling");
        assert_eq!(heat_note(Some(22.0), Some(0.0)), "target off");
        assert_eq!(heat_note(Some(22.0), None), "no target told");
        assert_eq!(heat_note(None, None), "not reported");
        assert!(flame("bed", Some(true)).contains(r#"data-on="true""#));
        assert!(flame("bed", None).contains(r#"data-on="""#));
    }

    /// #65: a device on an agent shows the agent's release (its `User-Agent-Version`, kept as
    /// the firmware) and whether it is connected now.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_device_on_an_agent_shows_its_release_and_state(pool: sqlx::PgPool) {
        let store = store::Store::for_test(&pool).await;
        let (space, id, token) = store
            .register_test_machine("LW450", maker_connect_protocol::MachineKind::Label)
            .await;
        let report = maker_connect_protocol::Telemetry {
            state: maker_connect_protocol::MachineState::Idle,
            firmware: Some("Maker Connect agent v0.3.0-v2ui.9".into()),
            ..Default::default()
        };
        store.report(&token, &report).await.unwrap();
        let machine = store.machine(space, &id).await.unwrap().unwrap();
        let card = connection(&machine);
        assert!(
            card.contains("<dt>Agent version</dt><dd>v0.3.0-v2ui.9</dd>"),
            "{card}"
        );
        let state = if machine.online {
            "Connected"
        } else {
            "Not connected"
        };
        assert!(
            card.contains(&format!("<dt>State</dt><dd>{state}</dd>")),
            "{card}"
        );
    }

    /// Step 3e, E11: a scale's page has one live line, status then weight and unit, as the scale says them.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_scale_shows_one_live_line(pool: sqlx::PgPool) {
        let store = store::Store::for_test(&pool).await;
        let (space, id, token) = store
            .register_test_machine("Postal", maker_connect_protocol::MachineKind::Scale)
            .await;
        let reading = |status: u8, text: &str| maker_connect_protocol::ScaleReading {
            status,
            status_text: text.into(),
            unit: 2,
            unit_name: "g".into(),
            exponent: -1,
            raw: 1234,
            weight: 123.4,
        };
        let report = |r| maker_connect_protocol::Telemetry {
            state: maker_connect_protocol::MachineState::Idle,
            scale: Some(r),
            ..Default::default()
        };
        store.report(&token, &report(reading(4, ""))).await.unwrap();
        let machine = store.machine(space, &id).await.unwrap().unwrap();
        let page = overview(&machine, None, "");
        assert!(
            page.contains(r#"<span data-live="scale">123.4 g</span>"#),
            "{page}"
        );
        store
            .report(&token, &report(reading(3, "Weighing...")))
            .await
            .unwrap();
        let machine = store.machine(space, &id).await.unwrap().unwrap();
        assert!(
            overview(&machine, None, "")
                .contains(r#"<span data-live="scale">Weighing... · 123.4 g</span>"#)
        );
    }

    /// A command's outcome is words on its own filled block (`.badge.finished`, `.badge.rejected`):
    /// no rule outside the job lists recolours a badge's words without its block, which left them
    /// in the block's own colour (green on green in the dark theme).
    #[test]
    fn a_badge_keeps_its_words_readable() {
        for rule in MACHINE_STYLE.split('}') {
            let Some((selector, body)) = rule.split_once('{') else {
                continue;
            };
            for one in selector.split(',') {
                let one = one.trim();
                if one.contains(".badge.") && !one.contains(".job-history") {
                    assert!(body.contains("background"), "{one} {{{body}}}");
                }
            }
        }
    }

    /// A printer on Prusa Connect has no bed mesh on its page: the protocol carries none. The
    /// same printer through an agent has it.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn no_bed_mesh_over_prusa_connect(pool: sqlx::PgPool) {
        let store = store::Store::for_test(&pool).await;
        let (space, agent, _) = store.add_test_machine("prusa/mini", None).await;
        let agent = store.machine(space, &agent).await.unwrap().unwrap();
        assert!(shows_bed_mesh(&agent), "through an agent: {}", agent.link);
        let (_, connect, _) = store
            .add_test_machine("prusa/mini", Some("prusa-connect"))
            .await;
        let connect = store.machine(space, &connect).await.unwrap().unwrap();
        assert!(!shows_bed_mesh(&connect));
    }

    /// Seen on the MINI (2026-11-08): a print started from its USB stick had no Stop
    /// anywhere. Whatever prints, the hub's job or the printer's own, has Stop.
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn every_print_has_stop(pool: sqlx::PgPool) {
        let store = store::Store::for_test(&pool).await;
        let (space, mini, token) = store.add_test_machine("prusa/mini", None).await;
        let actions = async |store: &store::Store| {
            let machine = store.machine(space, &mini).await.unwrap().unwrap();
            let card = job_card(&machine);
            let form = card
                .split("<form class=\"job-actions\"")
                .nth(1)
                .unwrap()
                .split('>')
                .next()
                .unwrap()
                .to_owned();
            (form, card)
        };
        // Idle: no job, so no buttons.
        let idle = maker_connect_protocol::Telemetry {
            state: maker_connect_protocol::MachineState::Idle,
            ..Default::default()
        };
        store.report(&token, &idle).await.unwrap();
        assert!(actions(&store).await.0.contains("hidden"));
        // Printing what the printer runs itself (no job of the hub's): Stop shown.
        for state in [
            maker_connect_protocol::MachineState::Busy,
            maker_connect_protocol::MachineState::Paused,
            maker_connect_protocol::MachineState::Attention,
        ] {
            let printing = maker_connect_protocol::Telemetry {
                state,
                job: Some(maker_connect_protocol::JobProgress {
                    file: String::new(),
                    done: 5,
                    total: 100,
                    elapsed_s: Some(752),
                    remaining_s: Some(3540),
                }),
                ..Default::default()
            };
            store.report(&token, &printing).await.unwrap();
            let (form, card) = actions(&store).await;
            assert!(!form.contains("hidden"), "{state:?}: {form}");
            assert!(card.contains(r#"value="stop""#));
            assert!(card.contains("a print the printer runs itself"));
        }
    }

    /// A laser's Stop (a soft reset) and its hint are there only while a job runs: with no
    /// job the hub answers 404 and with a loaded one 409 (files.rs control_job).
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn a_laser_has_stop_only_while_a_job_runs(pool: sqlx::PgPool) {
        let store = store::Store::for_test(&pool).await;
        let (space, laser, _) = store.add_test_machine("ortur/lm2-pro-s2", None).await;
        let mut machine = store.machine(space, &laser).await.unwrap().unwrap();
        let shown = |machine: &Machine| {
            let card = job_card(machine);
            let tag = |at: &str| {
                card.split(at)
                    .nth(1)
                    .unwrap()
                    .split('>')
                    .next()
                    .unwrap()
                    .to_owned()
            };
            (
                !tag("<form class=\"job-actions\"").contains("hidden"),
                !tag("data-laser-stop").contains("hidden"),
            )
        };
        assert_eq!(shown(&machine), (false, false), "idle: no Stop, no hint");
        let job = |state| store::Job {
            id: 1,
            machine_id: laser.clone(),
            file_id: "f".into(),
            file_name: "sign.gcode".into(),
            state,
            created_at: 0,
            started_at: None,
            ended_at: None,
            reason: None,
            from_line: None,
            restart_line: None,
            by: None,
        };
        machine.job = Some(job(store::JobState::Loaded));
        assert_eq!(shown(&machine), (false, false), "loaded: no Stop, no hint");
        machine.job = Some(job(store::JobState::Printing));
        assert_eq!(shown(&machine), (true, true), "running: Stop and hint");
        let card = job_card(&machine);
        assert!(card.contains(">Emergency stop (soft reset)</button>"));
        assert!(!card.contains("also while it frames"));
    }
}
