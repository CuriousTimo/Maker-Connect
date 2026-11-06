//! The files page: drop G-code to upload it, see what each file says about itself (its
//! picture, time, filament, the printer it was sliced for), and print it on a machine that
//! fits. Filled by its script from `/api/files`.

use crate::{STYLE, Shared, failed, header};
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};

const FILES_STYLE: &str = include_str!("files.css");
const FILES_SCRIPT: &str = include_str!("files.js");

pub async fn page(access: crate::access::Access, State(store): State<Shared>) -> Response {
    let space = match store.lock().unwrap().space_of(&access) {
        Ok((_, name)) => name,
        Err(error) => return failed(error),
    };
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Files · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{FILES_STYLE}</style>
</head>
<body>
{header}
<main class="wide">
  <div class="page-head">
    <div><h2>Files</h2><p class="subtitle">G-code and labels for your machines. Drop files anywhere on this page.</p></div>
    <div class="head-actions"><button type="button" class="ghost" data-examples title="3DBenchy sliced for the MK3S, the MINI, an Ender 3 and a CR-10: to try printing">Add example files</button><label class="upload-button"><input type="file" accept=".gcode,.gco,.g,.png" multiple hidden data-pick>Upload</label></div>
  </div>
  <div class="uploads" data-uploads></div>
  <div class="toolbar" data-toolbar>
    <input type="search" data-search placeholder="Search by name" aria-label="Search by name">
    <div class="kinds" role="group" aria-label="Type" data-kinds></div>
    <select data-machine aria-label="Machine"><option value="">All machines</option></select>
    <select data-sliced aria-label="Sliced for" hidden></select>
    <select data-material aria-label="Material" hidden></select>
    <select data-layer aria-label="Layer height" hidden></select>
    <select data-sort aria-label="Sort">
      <option value="new">Newest first</option>
      <option value="old">Oldest first</option>
      <option value="printed">Last printed</option>
      <option value="name">Name</option>
      <option value="size">Largest first</option>
    </select>
    <div class="views" role="group" aria-label="View">
      <button type="button" data-view="cards" title="Cards" aria-label="Cards"><svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1" y="1" width="6" height="6" rx="1"/><rect x="9" y="1" width="6" height="6" rx="1"/><rect x="1" y="9" width="6" height="6" rx="1"/><rect x="9" y="9" width="6" height="6" rx="1"/></svg></button>
      <button type="button" data-view="list" title="List" aria-label="List"><svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1" y="2" width="14" height="2.5" rx="1"/><rect x="1" y="6.75" width="14" height="2.5" rx="1"/><rect x="1" y="11.5" width="14" height="2.5" rx="1"/></svg></button>
    </div>
  </div>
  <div class="selection" data-selection>
    <span data-count></span>
    <button type="button" class="ghost" data-select-shown>Select all shown</button>
    <button type="button" class="ghost" data-select-old title="Shown files uploaded more than 30 days ago and not printed in that time">Select unprinted, older than 30 days</button>
    <button type="button" class="ghost" data-select-none hidden>Clear selection</button>
    <button type="button" class="danger" data-remove-selected hidden>Remove selected</button>
    <div class="storage" data-storage hidden title="What the files take, and what the hub's disk has left">
      <div class="storage-bar"><i data-storage-fill></i></div><span data-storage-text></span>
    </div>
  </div>
  <div class="files" data-files><p class="empty">Loading…</p></div>
  <div class="drop" data-drop hidden><div>Drop to upload</div></div>
  <dialog class="file-dialog" data-file-dialog aria-labelledby="file-title">
    <div class="dialog-head">
      <div class="file-dialog-title"><span class="kind" data-fd-kind></span><h2 id="file-title" data-fd-name></h2></div>
      <div class="file-dialog-nav">
        <button type="button" class="ghost" data-fd-prev title="Previous file (←)" aria-label="Previous file">‹</button>
        <button type="button" class="ghost" data-fd-next title="Next file (→)" aria-label="Next file">›</button>
        <button type="button" class="close" data-fd-close aria-label="Close">✕</button>
      </div>
    </div>
    <div class="file-dialog-body" data-fd-body data-id=""></div>
  </dialog>
</main>
<script>
{FILES_SCRIPT}</script>
</body>
</html>
"##,
        header = header(&space, "", "files"),
    ))
    .into_response()
}
