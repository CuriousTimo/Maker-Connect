//! A label printer's page: a label to make and print, and the printer's state. The label is
//! drawn as SVG (text, and drawings the owner adds) in the browser, at the tape's size; its
//! script (`label.js`) renders that at one pixel per dot, shows exactly the dots that will
//! burn, and sends them as a PNG: uploaded as a file, printed as a job.

use crate::machine_page;
use crate::profiles::{self, Profile};
use crate::store::Machine;
use crate::{STYLE, escape};

const MACHINE_STYLE: &str = include_str!("machine.css");
const LABEL_STYLE: &str = include_str!("label.css");
const LABEL_SCRIPT: &str = include_str!("label.js");

/// The tape loaded, as the script needs it: width, dots across, ink and tape colours.
fn tape_json(profile: Option<&Profile>, machine: &Machine) -> String {
    let media = profile.and_then(|p| profiles::media(p, machine.media.as_deref()?));
    let Some(media) = media else {
        return "null".to_owned();
    };
    let dots = maker_connect_label::dymo::Tape::d1(media.width_mm).map_or(0, |t| t.dots);
    serde_json::json!({
        "id": media.id,
        "name": media.name,
        "width_mm": media.width_mm,
        "dots": dots,
        "dpi": profile.and_then(|p| p.dpi).unwrap_or(180.0),
        "ink": media.ink.as_deref().unwrap_or("#000"),
        "background": media.background.as_deref().unwrap_or("#fff"),
    })
    .to_string()
    // Safe inside a <script> element.
    .replace('<', "\\u003c")
}

/// The tapes of the profile, by width, the loaded one chosen.
fn media_options(profile: Option<&Profile>, chosen: Option<&str>) -> String {
    let Some(profile) = profile else {
        return String::new();
    };
    let mut widths: Vec<u32> = profile.media.iter().map(|m| m.width_mm).collect();
    widths.sort_unstable();
    widths.dedup();
    let mut options = format!(
        r#"<option value=""{}>Not said</option>"#,
        if chosen.is_none() { " selected" } else { "" }
    );
    for width in widths {
        options += &format!(r#"<optgroup label="{width} mm">"#);
        for media in profile.media.iter().filter(|m| m.width_mm == width) {
            options += &format!(
                r#"<option value="{id}"{selected}>{name} ({id})</option>"#,
                id = escape(&media.id),
                name = escape(&media.name),
                selected = if chosen == Some(media.id.as_str()) {
                    " selected"
                } else {
                    ""
                },
            );
        }
        options += "</optgroup>";
    }
    options
}

pub fn render(space: &str, machine: &Machine) -> String {
    let profile = machine.profile.as_deref().and_then(profiles::find);
    let telemetry = machine.telemetry.as_ref().filter(|_| machine.online);
    let (state, state_class) = machine_page::shown_state(machine);
    let kind = profile.map_or(machine.kind.label(), |p| p.model.as_str());
    let link = match machine.link.as_str() {
        "virtual" => "Virtual, in the cloud",
        _ => "Maker Connect agent",
    };
    let id = escape(&machine.id);
    let tape_form = if profile.is_some_and(|p| !p.media.is_empty()) {
        format!(
            r#"<form method="post" action="{id}/media" class="tape-form"><select name="media" aria-label="Tape loaded" data-tape-select>{}</select><button>Save</button></form>
<p class="hint">The printer tells whether a cassette is in, not which: the article number is on the cassette.</p>"#,
            media_options(profile, machine.media.as_deref())
        )
    } else {
        r#"<p class="empty">Choose the printer's profile under Settings.</p>"#.to_owned()
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
{STYLE}{MACHINE_STYLE}{LABEL_STYLE}</style>
</head>
<body style="--page: 1240px">
{header}
<main class="wide" data-id="{id}">
  <a class="back" href="../">← All machines</a>
  <div class="title">
    <h2>{name}</h2>
    <span class="state {state_class}" data-live="state">{state}</span>
  </div>
  <p class="subtitle">{kind} · {link}</p>
  <nav class="tabs" role="tablist">
    <a href="#overview" role="tab">Overview</a>
    <a href="#label" role="tab">Label</a>
    <a href="#settings" role="tab">Settings</a>
  </nav>
  <section class="tab" id="overview" role="tabpanel">
{overview}
  </section>
  <section class="tab" id="label" role="tabpanel" hidden>
    <div class="grid-label">
      <div class="card">
        <div class="card-head"><div><h3>Label</h3><p class="card-sub">Drawn at the tape's size; the preview shows the dots that will burn</p></div></div>
        <div class="label-editor">
          <label class="field wide-field"><span>Text</span><textarea data-text rows="2" maxlength="200" placeholder="Maker Connect">Maker Connect</textarea><small>A new line for a second line</small></label>
          <label class="field"><span>Font</span><select data-font>
            <option value="system-ui, sans-serif">Sans</option>
            <option value="Georgia, serif">Serif</option>
            <option value="ui-monospace, Menlo, monospace">Mono</option>
            <option value="'Arial Narrow', 'Roboto Condensed', sans-serif-condensed, sans-serif">Narrow</option>
          </select></label>
          <label class="field"><span>Weight</span><select data-weight><option value="700">Bold</option><option value="400">Regular</option></select></label>
          <label class="field"><span>Size</span><input type="range" data-size min="40" max="100" value="100"><small data-size-note>fills the tape</small></label>
          <div class="field"><span>Style</span><div class="checks">
            <label><input type="checkbox" data-frame> Frame</label>
            <label><input type="checkbox" data-invert> Inverted</label>
          </div></div>
          <label class="field"><span>Code</span><select data-code-kind>
            <option value="none">None</option>
            <option value="qr">QR code</option>
            <option value="code128">Barcode (Code 128)</option>
            <option value="ean13">Barcode (EAN-13)</option>
          </select></label>
          <label class="field code-data"><span>What it encodes</span><input data-code-data maxlength="400" placeholder="The text, when empty" autocomplete="off"><small data-code-note></small></label>
          <div class="field wide-field"><span>Drawing</span>
            <div class="drawing-row"><input type="file" accept=".svg,image/svg+xml,image/png" data-drawing aria-label="Add an SVG drawing"><button type="button" class="ghost" data-drawing-remove hidden>Remove</button></div>
            <small>An SVG (or PNG), placed before the text at the tape's height</small></div>
        </div>
        <h4>Preview</h4>
        <div class="preview-wrap"><canvas data-preview aria-label="The label as it will print"></canvas></div>
        <p class="preview-note" data-preview-note></p>
        <div class="print-row"><button type="button" data-print>Print</button><span data-print-status role="status"></span></div>
        <details class="svg-source"><summary>The label's SVG</summary><pre data-svg></pre></details>
      </div>
      <div class="stack">
        <div class="card">
          <div class="card-head"><div><h3>Printer</h3><p class="card-sub">Live</p></div></div>
          <dl>
            <dt>State</dt><dd data-live="state">{state}</dd>
            <dt>Cassette</dt><dd data-live="cassette">{cassette}</dd>
            <dt>Job</dt><dd data-live="job">{job}</dd>
          </dl>
        </div>
        <div class="card">
          <div class="card-head"><div><h3>Tape loaded</h3><p class="card-sub">The label is made for it</p></div></div>
          {tape_form}
        </div>
      </div>
    </div>
  </section>
  <section class="tab" id="settings" role="tabpanel" hidden>
{settings}
  </section>
</main>
<script type="application/json" id="tape-data">{tape}</script>
<script>
{LABEL_SCRIPT}</script>
</body>
</html>
"##,
        name = escape(&machine.name),
        header = crate::header(space, "../", "machines"),
        state = escape(&state),
        kind = escape(kind),
        cassette = match telemetry.and_then(|t| t.cassette) {
            Some(true) => "Loaded",
            Some(false) => "Empty",
            None => "–",
        },
        job = machine
            .job
            .as_ref()
            .map_or("–".to_owned(), |j| escape(&j.file_name)),
        tape = tape_json(profile, machine),
        settings = machine_page::settings(machine),
        overview = machine_page::overview(machine, profile),
    )
}
