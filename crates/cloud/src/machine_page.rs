//! A machine's page: four tabs (overview, control, telemetry, settings) of tiles and cards,
//! rendered here and kept live by its script (`machine.js`) from the server-sent events.
//! Every value the script updates carries `data-live="<key>"`, rendered here the same way.

use crate::profiles::{self, Profile};
use crate::store::{self, Machine};
use crate::{STYLE, escape};
use maker_connect_protocol::MachineKind;

const MACHINE_STYLE: &str = include_str!("machine.css");
const MACHINE_SCRIPT: &str = include_str!("machine.js");

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
            _ => (t.state.label().to_owned(), t.state.label().to_lowercase()),
        },
    }
}

/// The whole page.
pub fn render(space: &str, machine: &Machine) -> String {
    let found = machine.profile.as_deref().and_then(profiles::find);
    // A laser without a profile: its work area from what its controller says.
    let told = found
        .is_none()
        .then(|| {
            let travel = machine.telemetry.as_ref()?.grbl.as_ref()?.travel_mm?;
            Some(profiles::from_controller(machine.kind, travel))
        })
        .flatten();
    let profile = found.or(told.as_ref());
    let (state, state_class) = shown_state(machine);
    let kind = profile.map_or(machine.kind.label(), |p| p.model.as_str());
    let link = match machine.link.as_str() {
        "prusa-connect" => "Built-in Prusa Connect",
        "virtual" => "Virtual, in the cloud",
        "simplyprint" => "Klipper, through Moonraker",
        _ => "Maker Connect agent",
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
<main class="wide" data-id="{id}">
  <div class="title">
    <h2>{name}</h2>
    <span class="state {state_class}" data-live="state">{state}</span>
  </div>
  <p class="subtitle">{kind} · {link}</p>
  <div class="notices" data-notices>{notices}</div>
  <nav class="tabs" role="tablist">
    <a href="#overview" role="tab">Overview</a>
    <a href="#control" role="tab">Control</a>
    <a href="#terminal" role="tab">Terminal</a>
    <a href="#telemetry" role="tab">Telemetry</a>
    {bed_tab}<a href="#settings" role="tab">Settings</a>
  </nav>
  <section class="tab" id="overview" role="tabpanel">
{overview}
  </section>
  <section class="tab" id="control" role="tabpanel" hidden>
{control}
  </section>
  <section class="tab" id="terminal" role="tabpanel" hidden>
{terminal}
  </section>
  <section class="tab" id="telemetry" role="tabpanel" hidden>
{telemetry}
  </section>
  {bed_level}
  <section class="tab" id="settings" role="tabpanel" hidden>
{settings}
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
        state = escape(&state),
        notices = notices(machine),
        kind = escape(kind),
        overview = overview(machine, profile),
        control = control(machine, profile),
        telemetry = telemetry_tab(machine),
        settings = settings(machine),
        terminal = terminal(machine),
        bed_tab = if heats(machine) {
            r##"<a href="#bedlevel" role="tab">Bed level</a>
    "##
        } else {
            ""
        },
        bed_level = if heats(machine) {
            format!(
                r#"<section class="tab" id="bedlevel" role="tabpanel" hidden>
{}
  </section>"#,
                bed_level(machine, profile)
            )
        } else {
            String::new()
        },
    )
}

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
    out
}

/// The bed level tab: the last mesh as numbers, and as a surface, a map and a table.
fn bed_level(machine: &Machine, profile: Option<&Profile>) -> String {
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
        Ok(_) if store::takes_commands(&machine.link) => format!(
            r#"<form class="command" method="post" action="{}/command"><button name="action" value="probe-bed">Probe bed</button></form>"#,
            escape(&machine.id)
        ),
        Ok(_) => String::new(),
        Err(why) => format!(r#"<p class="card-sub">{}</p>"#, escape(why)),
    };
    let [width, depth, _] = profile.map_or([200.0; 3], |p| p.build_volume_mm);
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
fn ago(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s ago"),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

fn heats(machine: &Machine) -> bool {
    machine.kind == MachineKind::Fdm
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
        r#"<div class="tile"><div class="label">{label}</div><div class="value"><span data-live="{key}">{}</span><small>{unit}</small></div><div class="sub" data-live="{sub_key}">{}</div></div>"#,
        escape(value.as_deref().unwrap_or("–")),
        escape(&sub)
    )
}

/// How a battery is doing, by its voltage: as the label page's script says it.
fn battery_note(volts: Option<f32>) -> String {
    use maker_connect_label::dymo::{BATTERY_LOW_V, BATTERY_PRINT_MIN_V};
    match volts {
        None => "not reported".to_owned(),
        Some(v) if v < BATTERY_PRINT_MIN_V => format!("{v:.2} V · too low to print"),
        Some(v) if v < BATTERY_LOW_V => format!("{v:.2} V · low"),
        Some(v) => format!("{v:.2} V · estimated"),
    }
}

/// A battery's tile: how full, as a percentage and a battery that fills, green, amber
/// under 30 %, red under 15 % (the label page's script keeps it live).
fn battery_tile(volts: Option<f32>) -> String {
    let percent = volts.map(maker_connect_label::dymo::battery_percent);
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
        note = escape(&battery_note(volts)),
    )
}

/// How a heater is doing, as the tiles say it (the script says it the same way).
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
    )
}

fn position_tiles(machine: &Machine) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let axis = |i: usize| {
        t.and_then(|t| t.position_mm)
            .map(|p| format!("{:.1}", p[i]))
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
fn job_now(machine: &Machine) -> (Option<String>, String) {
    let Some(job) = &machine.job else {
        return (None, "no job".to_owned());
    };
    let progress = machine.telemetry.as_ref().and_then(|t| t.job.as_ref());
    let percent = progress
        .filter(|p| p.total > 0)
        .map(|p| format!("{:.0}", p.done as f64 / p.total as f64 * 100.0));
    let left = progress
        .and_then(|p| p.remaining_s)
        .map(|s| format!(" · {} left", duration(s)))
        .unwrap_or_default();
    let state = serde_json::to_value(job.state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    (percent, format!("{state} · {}{left}", job.file_name))
}

fn job_tile(machine: &Machine) -> String {
    let (percent, note) = job_now(machine);
    tile("Job", "job_pct", percent, "%", "job_note", note)
}

/// The job card: the file, how far, how long, and pause, resume, stop.
fn job_card(machine: &Machine) -> String {
    let job = machine.job.as_ref();
    let progress = machine.telemetry.as_ref().and_then(|t| t.job.as_ref());
    let (percent, note) = job_now(machine);
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
  <div class="card-head"><div><h3>Job</h3><p class="card-sub" data-live="job_note">{note}</p></div></div>
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
  <form class="job-actions" data-job-actions action="{id}/job"{hidden}>
    <button name="action" value="pause">Pause</button>
    <button name="action" value="resume">Resume</button>
    <button name="action" value="stop" class="danger">Stop</button>
  </form>
  <p class="empty no-job" data-no-job{shown}>No job. <a href="../files">Print a file</a> on this machine.</p>
  <div class="resume" data-resume{resume_hidden}>
    <p><b>Interrupted:</b> <span data-live="resume_name">{resume_name}</span> ({resume_why}). It can start again from line <b data-live="resume_line">{resume_line}</b>, a little before where it stopped; the head goes there with the laser off first.</p>
    <form data-restart action="{id}/job"><button name="action" value="restart">Start again from there</button></form>
  </div>
</div>"#,
        note = escape(&note),
        hidden = if job.is_some() { "" } else { " hidden" },
        shown = if job.is_some() { " hidden" } else { "" },
        name = escape(job.map_or("", |j| j.file_name.as_str())),
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

/// A row the script keeps live; `–` while unknown.
fn live_row(label: &str, key: &str, value: Option<String>) -> String {
    format!(
        r#"<dt>{label}</dt><dd data-live="{key}">{}</dd>"#,
        escape(value.as_deref().unwrap_or("–"))
    )
}

pub(crate) fn overview(machine: &Machine, profile: Option<&Profile>) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let label = machine.kind == MachineKind::Label;
    let material = t
        .and_then(|t| t.material.clone())
        .or_else(|| machine.details.as_ref().and_then(|d| d.material.clone()));
    let tiles = if label {
        let media = profile.and_then(|p| profiles::media(p, machine.media.as_deref()?));
        tile(
            "Cassette",
            "cassette",
            t.and_then(|t| t.cassette)
                .map(|on| if on { "Loaded" } else { "Empty" }.to_owned()),
            "",
            "problem",
            t.and_then(|t| t.problem.clone()).unwrap_or_default(),
        ) + &tile(
            "Tape",
            "none",
            media.map(|m| m.width_mm.to_string()),
            "mm",
            "none",
            media.map_or("say which under Label".to_owned(), |m| {
                format!("{} · {}", m.name, m.id)
            }),
        ) + &battery_tile(t.and_then(|t| t.battery_v))
            + &tile(
                "Job",
                "job",
                machine.job.as_ref().map(|j| j.file_name.clone()),
                "",
                "none",
                String::new(),
            )
    } else if heats(machine) {
        heater_tiles(machine)
            + &match &machine.loaded {
                // The spool from Materials: what is left, and which it is.
                Some(l) => tile(
                    "Spool",
                    "none",
                    Some(if l.unit == "m" {
                        format!("{:.1}", l.amount)
                    } else {
                        format!("{:.0}", l.amount)
                    }),
                    &format!("{} left", l.unit),
                    "none",
                    format!("{} · {}", l.item, l.name),
                ),
                None => tile(
                    "Material",
                    "material",
                    material,
                    "",
                    "none",
                    profile
                        .and_then(|p| p.nozzle_mm)
                        .map_or(String::new(), |n| format!("{n} mm nozzle")),
                ),
            }
            + &job_tile(machine)
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
            + &job_tile(machine)
    };
    let picture = profile
        .and_then(Profile::image)
        .or_else(|| profiles::generic_image(machine.kind))
        .unwrap_or(r#"<div class="no-picture">No picture yet</div>"#);
    let facts = match profile {
        Some(p) => [
            row(
                "Profile",
                Some(format!(
                    "{}{}",
                    p.model,
                    if p.id.is_empty() {
                        ": $130 to $132; choose a profile under Settings"
                    } else if machine.profile_chosen {
                        ""
                    } else {
                        " (recognised)"
                    }
                )),
            ),
            row(
                if label {
                    "Takes"
                } else if p.build_volume_mm[2] > 0.0 {
                    "Build volume"
                } else {
                    "Work area"
                },
                Some(p.size()),
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
            row("Firmware", t.and_then(|t| t.firmware.clone())),
        ]
        .concat(),
        None => "<dt>Profile</dt><dd>Not known yet: choose one under Settings</dd>".to_owned(),
    };
    // What was asked of it: commands for a machine that takes G-code.
    let asked = if label {
        String::new()
    } else {
        card(
            "Latest commands",
            "What was asked of it from here, and how it went",
            &format!(
                r#"<ol class="commands" data-commands>{}</ol>"#,
                commands(machine)
            ),
        )
    };
    format!(
        r#"<div class="tiles">{tiles}</div>
<div class="grid-2">{}{asked}</div>"#,
        card(
            "Machine",
            "",
            &format!(
                r#"<div class="machine-card"><div class="picture">{picture}</div><dl>{facts}</dl></div>"#
            )
        ),
    )
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

fn control(machine: &Machine, profile: Option<&Profile>) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let position = t.and_then(|t| t.position_mm);
    let z_max = profile.map_or(0.0, |p| p.build_volume_mm[2]);
    let axis = |name: &str, key: &str, i: usize| {
        format!(
            r#"<span><small>{name}</small><b data-live="{key}">{}</b><small>mm</small></span>"#,
            position.map_or("–".to_owned(), |p| format!("{:.1}", p[i]))
        )
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
        let filled = position.map_or(0.0, |p| (p[2] as f64 / z_max * 100.0).clamp(0.0, 100.0));
        format!(
            r#"<div class="zgauge" data-z-max="{z_max}" title="Z, of {z_max} mm"><span>{z_max}</span><div class="ztrack"><i data-z-fill style="height: {filled:.1}%"></i></div><span>0</span></div>"#
        )
    } else {
        String::new()
    };
    let bed = card(
        if heats(machine) { "Bed" } else { "Work area" },
        "The head where the machine says it is, live",
        &profile.map_or(
            r#"<p class="empty">Choose a profile under Settings to see the bed.</p>"#.to_owned(),
            |p| {
                format!(
                    r#"<div class="xyz">{xyz}</div><div class="bed-row">{}{gauge}</div>"#,
                    profiles::bed_svg(p)
                )
            },
        ),
    );
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
    let axis = |name: &str| {
        let lower = name.to_lowercase();
        format!(
            "<span>{name}</span>{}{}",
            button(&format!("{lower}-"), "−10"),
            button(&format!("{lower}+"), "+10")
        )
    };
    let fdm = heats(machine);
    let mut controls = if fdm {
        format!(
            r#"<h4>Move</h4><div class="jog">{}{}{}{}</div><p class="hint">10 mm from where the head is. Home first.</p>"#,
            button("home", "Home all"),
            axis("X"),
            axis("Y"),
            axis("Z")
        )
    } else {
        jog_pad()
    };
    if fdm {
        controls += &format!(
            r#"<h4>Heat</h4><div class="buttons">{}{}{}</div>{}<h4>Fan and motors</h4><div class="buttons">{}{}{}</div>"#,
            button("preheat-pla", "Preheat PLA"),
            button("preheat-petg", "Preheat PETG"),
            button("cooldown", "Cool down"),
            heat_inputs(t, profile) + &factor_inputs(t),
            button("fan-on", "Print fan on"),
            button("fan-off", "Print fan off"),
            button("motors-off", "Motors off"),
        );
    } else {
        controls += &format!(
            r#"<h4>Origin</h4><div class="buttons">{}{}</div><p class="hint">A job starts from its origin. Move the head to the corner of what you work on, then set it; the machine forgets it when it restarts.</p><h4>Machine</h4><div class="buttons">{}{}</div><p class="hint">Unlock after an alarm ($X); Laser off switches the beam off (M5).</p>"#,
            button("set-origin", "Set origin here"),
            button("go-origin", "Go to origin"),
            button("unlock", "Unlock"),
            button("laser-off", "Laser off")
        );
    }
    let form = format!(
        r#"<form class="command" method="post" action="{}/command">{controls}</form><p class="note" data-command-status role="status"></p>"#,
        escape(&machine.id)
    );
    format!(
        r#"<div class="grid-control">{bed}<div class="stack">{}{}{}</div></div>"#,
        job_card(machine),
        card(
            "Control",
            "Sent one at a time; each shows below how it went",
            &form
        ),
        card(
            "Commands",
            "",
            &format!(
                r#"<ol class="commands" data-commands>{}</ol>"#,
                commands(machine)
            )
        )
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

fn telemetry_tab(machine: &Machine) -> String {
    let t = machine.telemetry.as_ref().filter(|_| machine.online);
    let rpm = |v: Option<u32>| v.map(|v| v.to_string());
    let percent = |v: Option<f32>| v.map(|v| format!("{v} %"));
    let motion = [
        live_row(
            "Position",
            "position",
            t.and_then(|t| t.position_mm)
                .map(|[x, y, z]| format!("{x:.1} · {y:.1} · {z:.1} mm")),
        ),
        live_row("Speed", "speed", percent(t.and_then(|t| t.speed_percent))),
    ]
    .concat();
    if !heats(machine) {
        return format!(
            r#"<div class="tiles">{}</div><div class="grid-2">{}</div>"#,
            position_tiles(machine),
            card("Motion", "", &format!("<dl>{motion}</dl>"))
        );
    }
    let fans = tile(
        "Print fan",
        "print_fan_n",
        rpm(t.and_then(|t| t.print_fan_rpm)),
        "rpm",
        "none",
        "reported while printing".to_owned(),
    ) + &tile(
        "Hotend fan",
        "hotend_fan_n",
        rpm(t.and_then(|t| t.hotend_fan_rpm)),
        "rpm",
        "none",
        "reported while printing".to_owned(),
    );
    let printing = [
        live_row("Flow", "flow", percent(t.and_then(|t| t.flow_percent))),
        live_row("Material", "material", t.and_then(|t| t.material.clone())),
    ]
    .concat();
    let chart = r#"<div class="card chart-card">
  <div class="card-head">
    <div><h3>Temperatures</h3><p class="card-sub">The last 30 minutes, live · targets dashed</p></div>
    <button type="button" class="ghost" data-table-toggle aria-pressed="false">Table</button>
  </div>
  <div class="legend"><span><i class="key key-nozzle"></i>Nozzle</span><span><i class="key key-bed"></i>Bed</span><span><i class="key key-target"></i>Target</span></div>
  <div class="plot">
    <svg id="chart" role="img" aria-label="Nozzle and bed temperature over the last 30 minutes"></svg>
    <div id="tip" class="tip" hidden></div>
  </div>
  <div class="table-wrap" data-table hidden></div>
</div>"#;
    format!(
        r#"<div class="tiles">{}{fans}</div>
{chart}
<div class="grid-2">{}{}</div>"#,
        heater_tiles(machine),
        card("Motion", "", &format!("<dl>{motion}</dl>")),
        card("Printing", "", &format!("<dl>{printing}</dl>")),
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
    let read = if store::takes_commands(&machine.link) {
        format!(
            r#"<form class="command" method="post" action="{}/command"><button name="action" value="settings" class="ghost">Read them again</button></form><p class="note" data-command-status role="status"></p>"#,
            escape(&machine.id)
        )
    } else {
        String::new()
    };
    card(
        "Controller",
        "Its GRBL settings, as it reported them ($$)",
        &format!("<dl>{rows}</dl>{notes}{read}"),
    )
}

/// How a device reaches the cloud, the same card for every kind: the way (an agent, its own
/// firmware, simulated), the agent's computer as it tells, the address its reports come
/// from, and when the last came.
fn connection(machine: &Machine) -> String {
    let way = match machine.link.as_str() {
        "prusa-connect" => "Its own firmware (Prusa Connect)",
        "virtual" => "Virtual: simulated in the cloud",
        "simplyprint" => "Moonraker, by itself (its SimplyPrint component)",
        _ => "Maker Connect agent",
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
    let rows = [
        row("Connected by", Some(way.to_owned())),
        row(
            "Agent version",
            agent
                .map(|a| match (&a.build, &a.built) {
                    (Some(build), Some(built)) => format!("{} · build {build}, {built}", a.version),
                    (Some(build), None) => format!("{} · build {build}", a.version),
                    _ => format!("{} (an agent from before builds were named)", a.version),
                })
                .or_else(|| (machine.link == "agent").then(|| machine.agent_version.clone())),
        ),
        row(
            "Update",
            machine
                .agent_update
                .as_ref()
                .map(|b| format!("build {b} is on the Downloads page")),
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
url: ws://…/simplyprint/{token}</pre><p class="hint" data-moonraker-local hidden>This page was opened as <b>localhost</b>: the printer cannot reach that. Open it by this computer's address on the network, or put that address in the line yourself.</p><p class="hint">It takes the place of SimplyPrint's own cloud on that printer: a printer has one.</p>"#,
                token = escape(token)
            )
        });
    card(
        "Connection",
        "How it reaches the cloud",
        &format!("<dl>{rows}</dl>{moonraker}"),
    )
}

pub(crate) fn settings(machine: &Machine) -> String {
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
    let current = match machine.profile.as_deref().and_then(profiles::find) {
        Some(p) => format!(
            r#"<div class="current-profile"><div class="current-picture">{picture}</div><div><b>{brand} {model}</b><small>{size}{firmware}</small><span class="profile-how">{how}</span></div></div>"#,
            picture = p.image().unwrap_or(""),
            brand = escape(&p.brand),
            model = escape(&p.model),
            size = escape(&p.size()),
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
            how = if machine.profile_chosen {
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
            r#"<form method="post" action="{id}/out-of-use" data-async data-done="Out of use" data-then="reload"><input type="hidden" name="action" value="out"><input name="reason" maxlength="200" placeholder="Why: nozzle clogged, waiting for a part" aria-label="Why it is out of use"><button class="ghost">Take out of use</button></form><p class="hint">A machine out of use takes no jobs; commands still reach it, to mend it.</p>"#
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
    let forms = format!(
        r#"<h4>Name</h4>
<form method="post" action="{id}/name" data-async data-done="Renamed" data-rename><input name="name" value="{name}" aria-label="Name" maxlength="60" required><button>Rename</button></form>
<h4>Profile</h4>
{current}
<details class="change-profile"><summary class="ghost-button">Change profile</summary>
<div class="picker in-card" data-picker data-action="{id}/profile" data-selected="{chosen}" data-auto="{auto}"></div>
</details>
{laser}<h4>In use</h4>
{in_use}
<h4>Remove</h4>
<form method="post" action="{id}/remove" data-async data-done="Machine removed" data-confirm="Remove this machine from the space?" data-then="../"><button class="danger">Remove machine</button></form>"#
    );
    let about = [
        row("Serial number", details.serial.clone()),
        row("Printer type", details.printer_type.clone()),
        row(
            "Firmware",
            details
                .firmware
                .clone()
                .or_else(|| machine.telemetry.as_ref().and_then(|t| t.firmware.clone())),
        ),
        row("Nozzle", details.nozzle_mm.map(|n| format!("{n} mm"))),
        row(
            "Status bytes",
            machine
                .telemetry
                .as_ref()
                .filter(|_| machine.online)
                .and_then(|t| t.raw_status.clone()),
        ),
    ]
    .concat();
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
    if !about.is_empty() {
        info += &card(
            "Machine",
            "What it told about itself",
            &format!("<dl>{about}</dl>"),
        );
    }
    if !storage.is_empty() {
        info += &card("Storage", "", &format!("<dl>{storage}</dl>"));
    }
    if !network.is_empty() {
        info += &card("Network", "", &format!("<dl>{network}</dl>"));
    }
    format!(
        r#"<div class="grid-settings">{}<div class="stack">{info}</div></div>"#,
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
        r#"<h4>Move</h4><div class="jogpad" tabindex="0" data-jogpad aria-label="Move the head: arrow keys move by the step">
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
}
