//! A DYMO LabelWriter's page (HS2, `docs/briefs/hs2-lw450-label-page.md`): its own page, apart from
//! the LabelManager's (`label_page.rs`), as the design the owner reviewed (2026-11-17,
//! <https://claude.ai/artifact/8BJZKGpNjQzwJsJ6GPNSWU>). Ours, UI only.
//!
//! The label is a page of exactly the loaded roll's raster as the printer's port gives it
//! (`maker_connect_engine::ports::geometry::lw450`: `cupsWidth` × `cupsHeight` dots, the
//! imageable area at 300 dpi, width across the head). The script (`labelwriter.js`) draws it as
//! SVG at that size, renders it at one pixel per dot and sends it as a PNG: uploaded as a file,
//! printed as a job (the job tells the agent the roll's length). Nothing is rotated or scaled
//! after that: the port hands the PNG to DYMO's filter as it is.

use crate::machine_page;
use crate::profiles::{self, Profile};
use crate::store::Machine;
use crate::{STYLE, escape};

const MACHINE_STYLE: &str = include_str!("../machine.css");
const LABELWRITER_STYLE: &str = include_str!("../labelwriter.css");
const LABELWRITER_SCRIPT: &str = include_str!("../labelwriter.js");

/// The roll loaded, as the script needs it: the page's raster and the PPD's paper and imageable
/// area (points), all from the port's geometry.
pub(crate) fn roll_json(profile: Option<&Profile>, machine: &Machine) -> String {
    let Some(media) = machine.media.as_deref() else {
        return "null".to_owned();
    };
    let Some(roll) = profiles::labelwriter_roll(profile, media) else {
        return "null".to_owned();
    };
    let page = &roll.page;
    serde_json::json!({
        "id": roll.roll.sku,
        "name": roll.roll.name,
        "paper_name": roll.roll.paper,
        "page_size": page.name,
        "width_mm": page.width_mm,
        "length_mm": page.length_mm,
        "dots": page.dots,
        "paper": page.paper_dimension,
        "imageable": page.imageable_area,
        // DYMO Connect's labels on the page, in the raster's dots (HS2b)
        "cells": roll.cells,
    })
    .to_string()
    // Safe inside a <script> element.
    .replace('<', "\\u003c")
}

/// The rolls the printer takes, each a button of the media form; the search box filters them.
fn roll_list(profile: Option<&Profile>, chosen: Option<&str>) -> String {
    let Some(profile) = profile else {
        return r#"<p class="empty">Choose the printer's profile under Settings.</p>"#.to_owned();
    };
    let mut out = String::new();
    for media in &profile.media {
        let Some(roll) = profiles::labelwriter_roll(Some(profile), &media.id) else {
            continue;
        };
        let [pw, pl] = roll.page.paper_dimension;
        // The page as the preview shows it (width across the head), longer side 40 px, with its labels.
        let k = 40.0 / pw.max(pl);
        let (sw, sh) = ((pw * k).round().max(6.0), (pl * k).round().max(6.0));
        let n = roll.cells.len();
        out += &format!(
            r#"<button name="media" value="{id}" class="roll" aria-pressed="{pressed}" data-search="{search}"><span class="roll-shape"><span style="width: {sw}px; height: {sh}px">{split}</span></span><span class="roll-text"><b>{id}</b> {name}<small>{w} × {l} mm{up} · {paper}</small></span></button>"#,
            id = escape(&media.id),
            pressed = chosen == Some(media.id.as_str()),
            search = escape(
                &format!(
                    "{} {} {} {}",
                    roll.roll.sku,
                    roll.roll.name.unwrap_or(""),
                    roll.roll.paper,
                    roll.roll.search
                )
                .to_lowercase()
            ),
            name = escape(roll.roll.name.unwrap_or("")),
            w = roll.page.width_mm,
            l = roll.page.length_mm,
            up = if n > 1 {
                format!(", {n} labels")
            } else {
                String::new()
            },
            paper = escape(roll.roll.paper),
            split = if n > 1 {
                r#"<i class="roll-split"></i>"#
            } else {
                ""
            },
        );
    }
    out
}

pub fn render(
    space: &str,
    machine: &Machine,
    move_to: &[(String, String)],
    manage: bool,
) -> String {
    let profile = machine.profile.as_deref().and_then(profiles::find);
    let telemetry = machine.telemetry.as_ref().filter(|_| machine.online);
    let (state, state_class) = machine_page::shown_state(machine);
    let kind = profile.map_or(machine.kind.label(), |p| p.model.as_str());
    let link = match machine.link.as_str() {
        "virtual" => "Virtual, in the hub",
        _ => "Maker Connect agent",
    };
    let id = escape(&machine.id);
    let count = profile.map_or(0, |p| p.media.len());
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name} · Maker Connect</title>
<link rel="icon" href="../logo.svg" type="image/svg+xml">
<style>
{STYLE}{MACHINE_STYLE}{LABELWRITER_STYLE}</style>
</head>
<body>
{header}
<main class="wide" data-id="{id}">
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
    <div class="lw-grid">
      <div class="card">
        <div class="card-head"><div><h3>Label</h3><p class="card-sub" data-roll-line>Choose the roll loaded</p></div></div>
        <div class="lw-preview">
          <p class="lw-out" data-out hidden><svg width="14" height="18" viewBox="0 0 14 18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M7 17V2M1 8l6-6 6 6"></path></svg>Out of the printer this way: the top edge first, seen from the front</p>
          <p class="lw-empty" data-preview-empty>Choose the roll loaded, on the right: the page is drawn at its size.</p>
          <div class="lw-paper" data-paper hidden><canvas data-preview aria-label="The page as it will print"></canvas></div>
          <div class="lw-legend" data-legend hidden><span><i class="k-paper"></i>Label (PPD PaperDimension)</span><span><i class="k-area"></i>Printable area (ImageableArea), the page sent</span><span data-two-legend hidden><i class="k-split"></i>The labels on the page (DYMO Connect)</span></div>
        </div>
        <dl class="lw-facts">
          <div><dt>Page</dt><dd data-fact-page>–</dd></div>
          <div><dt>Printable</dt><dd data-fact-dots>–</dd></div>
          <div><dt>From</dt><dd data-fact-from>DYMO's PPD</dd></div>
        </dl>
        <p class="lw-note" data-two-note hidden>DYMO's driver prints this roll as one page; DYMO Connect's catalogue says which labels are on it. Where they lie on the page is not yet verified on the printer.</p>
        <p class="lw-note">Shown as the label comes out of the printer, seen from the front: the page's first row leaves first, its width runs across the head (seen on the printer, 2026-11-17).</p>
        <div class="lw-editor">
          <label class="field wide"><span data-text-label>Text</span><textarea data-text rows="2" maxlength="200">Maker Connect</textarea><small>A new line for a second line</small></label>
          <label class="field wide" data-second hidden><span>Second label</span><textarea data-text2 rows="2" maxlength="200"></textarea></label>
          <div class="field" data-two-field hidden><span>The two labels</span><div class="pills"><button type="button" class="pill" data-both="same" aria-pressed="true">Same</button><button type="button" class="pill" data-both="different" aria-pressed="false">Different</button></div></div>
          <div class="field"><span>Text size</span><div class="pills" data-size-pills><button type="button" class="pill" data-size="0.5" aria-pressed="false">S</button><button type="button" class="pill" data-size="0.75" aria-pressed="true">M</button><button type="button" class="pill" data-size="1" aria-pressed="false">L</button></div></div>
          <div class="field"><span>Align</span><div class="pills" data-align-pills><button type="button" class="pill" data-align="start" aria-pressed="false">Left</button><button type="button" class="pill" data-align="middle" aria-pressed="true">Centre</button></div></div>
          <div class="field"><span>Turn</span><div class="pills" data-turn-pills><button type="button" class="pill" data-turn="0" aria-pressed="true">0°</button><button type="button" class="pill" data-turn="90" aria-pressed="false">90°</button><button type="button" class="pill" data-turn="180" aria-pressed="false">180°</button><button type="button" class="pill" data-turn="270" aria-pressed="false">270°</button></div></div>
          <div class="field"><span>Shift</span><div class="pills"><button type="button" class="pill" data-shift="-1" aria-label="Shift up 0.25 mm">↑</button><button type="button" class="pill" data-shift="1" aria-label="Shift down 0.25 mm">↓</button><button type="button" class="pill" data-shift-x="-1" aria-label="Shift left 0.25 mm">←</button><button type="button" class="pill" data-shift-x="1" aria-label="Shift right 0.25 mm">→</button><button type="button" class="pill" data-shift="0">Reset</button></div><small data-shift-note>Centred on each label</small></div>
          <label class="field"><span>Code</span><select data-code-kind>
            <option value="none">None</option>
            <option value="qr">QR code</option>
            <option value="code128">Barcode (Code 128)</option>
            <option value="ean13">Barcode (EAN-13)</option>
          </select></label>
          <label class="field wide" data-code-field hidden><span>What it encodes</span><input data-code-data maxlength="400" placeholder="The text, when empty" autocomplete="off"><small data-code-note></small></label>
        </div>
        <div class="lw-print"><label><span>Copies</span><input type="number" data-copies min="1" max="50" value="1"></label><button type="button" data-print>Print</button><span data-print-status role="status"></span></div>
      </div>
      <div class="stack">
        <div class="card">
          <div class="card-head"><div><h3>Printer</h3><p class="card-sub">Live</p></div></div>
          <dl>
            <dt>State</dt><dd data-live="state">{state}</dd>
            <dt>Roll</dt><dd data-live="roll">{held}</dd>
            <dt>Job</dt><dd data-live="job">{job}</dd>
          </dl>
        </div>
        <div class="card">
          <div class="card-head"><div><h3>Roll loaded</h3><p class="card-sub">The page is made for it</p></div></div>
          <p class="hint">The printer says only when the labels have run out, not which roll is in: the number is on the roll's box. Click the one loaded.</p>
          <label class="lw-find"><span>Find a roll</span><input type="search" data-roll-search placeholder="Number on the box or size, e.g. S0722530 or 13x25" autocomplete="off"></label>
          <p class="hint" data-roll-count>{count} rolls in DYMO Connect's catalogue for this printer</p>
          <form method="post" action="{id}/media" class="lw-rolls" data-async data-done="Labels changed" data-roll-form>{rolls}</form>
        </div>
      </div>
    </div>
  </section>
  <section class="tab" id="settings" role="tabpanel" hidden>
{settings}
  </section>
</main>
<script type="application/json" id="roll-data">{roll}</script>
<script>
{LABELWRITER_SCRIPT}</script>
</body>
</html>
"##,
        name = escape(&machine.name),
        header = crate::header(space, "../", "machines"),
        state = escape(&state),
        kind = escape(kind),
        // A LabelWriter says only when its labels have run out.
        held = match telemetry {
            None => "–",
            Some(t)
                if t.problem
                    .as_deref()
                    .is_some_and(|p| p.starts_with("no labels")) =>
                "Empty",
            Some(_) => "Loaded",
        },
        job = machine
            .job
            .as_ref()
            .map_or("–".to_owned(), |j| escape(&j.file_name)),
        rolls = roll_list(profile, machine.media.as_deref()),
        roll = roll_json(profile, machine),
        settings = machine_page::settings(machine, move_to, false, manage),
        overview = machine_page::overview(machine, profile, ""),
    )
}
