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

pub async fn page(State(store): State<Shared>) -> Response {
    let space = match store.lock().unwrap().default_space() {
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
<body style="--page: 1240px">
{header}
<main class="wide">
  <div class="page-head">
    <div><h2>Files</h2><p class="subtitle">G-code and labels for your machines. Drop files anywhere on this page.</p></div>
    <label class="upload-button"><input type="file" accept=".gcode,.gco,.g,.png" multiple hidden data-pick>Upload</label>
  </div>
  <div class="uploads" data-uploads></div>
  <div class="toolbar" data-toolbar>
    <input type="search" data-search placeholder="Search by name" aria-label="Search by name">
    <div class="kinds" role="group" aria-label="Type" data-kinds></div>
    <select data-machine aria-label="Machine"><option value="">All machines</option></select>
    <select data-sort aria-label="Sort">
      <option value="new">Newest first</option>
      <option value="old">Oldest first</option>
      <option value="printed">Last printed</option>
      <option value="name">Name</option>
      <option value="size">Largest first</option>
    </select>
  </div>
  <div class="selection" data-selection>
    <span data-count></span>
    <button type="button" class="ghost" data-select-shown>Select all shown</button>
    <button type="button" class="ghost" data-select-old title="Shown files uploaded more than 30 days ago and not printed in that time">Select unprinted, older than 30 days</button>
    <button type="button" class="ghost" data-select-none hidden>Clear selection</button>
    <button type="button" class="danger" data-remove-selected hidden>Remove selected</button>
  </div>
  <div class="files" data-files><p class="empty">Loading…</p></div>
  <div class="drop" data-drop hidden><div>Drop to upload</div></div>
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
