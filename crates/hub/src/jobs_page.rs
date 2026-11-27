//! Jobs: every job of the space that ran, on every machine (D-F, #71). `GET /api/v1/jobs` is Prusa
//! Connect's list (`reference/prusa-connect` `2e8c62b`, `internal/transport/http/httpapi/jobs.go`), with our
//! additions where Connect has no word (`docs/design/api.md`, "The jobs list"); the page reads it
//! (wiki Design: The Website, "Jobs").

use crate::access::Access;
use crate::store::{Job, JobQuery, JobState};
use crate::{STYLE, Store, escape, failed, header};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use maker_connect_protocol::MachineKind;
use serde::Deserialize;
use serde_json::{Value, json};

/// Connect's page size when none is asked for, and its cap (`internal/app/devicejobs.go:652-653`).
const DEFAULT_PAGE: usize = 50;
const MAX_PAGE: usize = 200;

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/jobs", get(page))
        .route("/api/v1/jobs", get(list))
}

/// The query of `GET /api/v1/jobs`: Connect's (`jobs.go:66-81`), then ours.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ListQuery {
    device_id: Option<String>,
    phase: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
    person: Option<String>,
    file: Option<String>,
    outcome: Option<String>,
    kind: Option<String>,
}

/// A refusal in the stable error body (`api.md`, Conventions).
fn invalid(text: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "code": "request/INVALID", "text": text })),
    )
        .into_response()
}

/// RFC 3339 (`2026-11-27T10:00:00Z`, or with `+01:00`) as Unix seconds.
fn instant(text: &str) -> Option<i64> {
    let text = text.trim();
    let (base, offset) = match text.strip_suffix('Z').or_else(|| text.strip_suffix('z')) {
        Some(base) => (base, 0),
        None => {
            let at = text.len().checked_sub(6)?;
            let (base, zone) = text.split_at(at);
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (h, m) = zone[1..].split_once(':')?;
            (
                base,
                sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60),
            )
        }
    };
    base.contains('T')
        .then(|| crate::spoolman::parse_iso(base))
        .flatten()
        .map(|t| t - offset)
}

/// The states a job that ended in this `outcome` is kept in.
fn outcome_states(outcome: &str) -> Option<Vec<String>> {
    Some(match outcome {
        "finished" => vec!["finished".into()],
        "cancelled" => vec!["stopped".into()],
        "failed" => vec!["failed".into()],
        // Connect's fourth: none of ours ends so.
        "unknown" => vec![],
        _ => return None,
    })
}

/// Connect's `outcome` of a job that ended.
fn outcome(state: JobState) -> Option<&'static str> {
    match state {
        JobState::Finished => Some("finished"),
        JobState::Stopped => Some("cancelled"),
        JobState::Failed => Some("failed"),
        _ => None,
    }
}

/// Ours: what kind of job a machine's kind makes.
fn kind_of(kind: MachineKind) -> &'static str {
    match kind {
        MachineKind::Fdm => "print",
        MachineKind::Label => "label",
        MachineKind::Laser => "burn",
        // A CNC, a cutter, ...: no jobs list word of ours yet.
        _ => "other",
    }
}

/// The machine kinds (as kept) of a job kind.
fn kinds_of(kind: &str) -> Option<Vec<String>> {
    let machine = match kind {
        "print" => MachineKind::Fdm,
        "label" => MachineKind::Label,
        "burn" => MachineKind::Laser,
        _ => return None,
    };
    Some(vec![serde_json::to_string(&machine).ok()?])
}

/// The query as the store takes it, or the refusal.
pub(crate) fn query_of(q: &ListQuery) -> Result<JobQuery, &'static str> {
    let ended = match q.phase.as_deref() {
        None | Some("") => None,
        Some("running") => Some(false),
        Some("ended") => Some(true),
        Some(_) => return Err("phase: running or ended"),
    };
    let time = |t: &Option<String>| match t.as_deref() {
        None | Some("") => Ok(None),
        Some(t) => instant(t).map(Some).ok_or("from and to: RFC 3339"),
    };
    let (from, to) = (time(&q.from)?, time(&q.to)?);
    // Connect's refusals (jobs.go:113-120).
    if ended == Some(false) && (from.is_some() || to.is_some()) {
        return Err("from and to bound when a job ended, and a running job has not ended");
    }
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return Err("from is after to");
    }
    let after = match q.cursor.as_deref() {
        None | Some("") => None,
        Some(c) => Some(
            c.split_once('.')
                .and_then(|(s, i)| Some((s.parse().ok()?, i.parse().ok()?)))
                .ok_or("cursor: as a previous page returned it")?,
        ),
    };
    let some = |t: &Option<String>| t.clone().filter(|t| !t.is_empty());
    Ok(JobQuery {
        machine: some(&q.device_id),
        ended,
        from,
        to,
        person: some(&q.person),
        file: some(&q.file),
        states: match some(&q.outcome) {
            None => None,
            Some(o) => {
                Some(outcome_states(&o).ok_or("outcome: finished, cancelled, failed or unknown")?)
            }
        },
        kinds: match some(&q.kind) {
            None => None,
            Some(k) => Some(kinds_of(&k).ok_or("kind: print, label or burn")?),
        },
        after,
        limit: match q.limit {
            None | Some(0) => DEFAULT_PAGE,
            Some(n) => n.min(MAX_PAGE),
        },
    })
}

/// One row as Connect's `DeviceJob` (`httpapi/devicejobs.go:57-75`), with ours (`kind`, `by`).
/// `progress` and `time_remaining_ms` are what the machine says of its running job; a job that
/// finished reads 100, as Connect's does; else 0. `object_count` and `objects` are left out.
fn row(job: &Job, kind: MachineKind, live: Option<&maker_connect_protocol::JobProgress>) -> Value {
    let running = job.ended_at.is_none();
    let (progress, remaining_ms) = match (running, live) {
        (true, Some(p)) if p.total > 0 => (
            (p.done as f64 * 100.0 / p.total as f64).floor(),
            p.remaining_s.unwrap_or(0) * 1000,
        ),
        _ if job.state == JobState::Finished => (100.0, 0),
        _ => (0.0, 0),
    };
    let mut row = json!({
        "id": job.id.to_string(),
        "device_job_id": job.id.to_string(),
        "device_id": job.machine_id,
        "path": job.file_name,
        "display_name": job.file_name,
        "state": job.state.name(),
        "progress": progress,
        "time_remaining_ms": remaining_ms,
        "started": crate::spoolman::iso(job.started_at.unwrap_or(job.created_at)),
        "kind": kind_of(kind),
        "by": job.by,
        "file": job.file_id,
    });
    if let Some(ended) = job.ended_at {
        row["ended"] = json!(crate::spoolman::iso(ended));
    }
    if let Some(outcome) = outcome(job.state) {
        row["outcome"] = json!(outcome);
    }
    row
}

/// `GET /api/v1/jobs`: one page, `{items, cursor, limit}` as Connect's `JobPage` (`jobs.go`); the
/// cursor is empty on the last page. Every member views every machine of the space (`grants_for`,
/// view), so the space's rows are the caller's.
async fn list(access: Access, State(store): State<Store>, Query(q): Query<ListQuery>) -> Response {
    let query = match query_of(&q) {
        Ok(query) => query,
        Err(text) => return invalid(text),
    };
    let jobs = match store.job_list(access.space, &query).await {
        Ok(jobs) => jobs,
        Err(error) => return failed(error),
    };
    let machines = match store.machines(access.space).await {
        Ok(machines) => machines,
        Err(error) => return failed(error),
    };
    let live = |id: &str| {
        machines
            .iter()
            .find(|m| m.id == id)
            .and_then(|m| m.telemetry.as_ref()?.job.as_ref())
    };
    let cursor = match jobs.last() {
        Some((job, _)) if jobs.len() == query.limit => {
            format!("{}.{}", job.started_at.unwrap_or(job.created_at), job.id)
        }
        _ => String::new(),
    };
    let items: Vec<Value> = jobs
        .iter()
        .map(|(job, kind)| row(job, *kind, live(&job.machine_id)))
        .collect();
    Json(json!({ "items": items, "cursor": cursor, "limit": query.limit })).into_response()
}

async fn page(access: Access, State(store): State<Store>) -> Response {
    let space = match store.space_name(access.space).await {
        Ok(name) => name,
        Err(error) => return failed(error),
    };
    let machines = match store.machines(access.space).await {
        Ok(machines) => machines,
        Err(error) => return failed(error),
    };
    let options: String = machines
        .iter()
        .filter(|m| m.kind != MachineKind::Cameras)
        .map(|m| {
            format!(
                r#"<option value="{}">{}</option>"#,
                escape(&m.id),
                escape(&m.name)
            )
        })
        .collect();
    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Jobs · Maker Connect</title>
<link rel="icon" href="logo.svg" type="image/svg+xml">
<style>
{STYLE}{JOBS_STYLE}
</style>
</head>
<body>
{header}
<main id="main" tabindex="-1" class="wide">
<div class="page-head"><div><h2>Jobs</h2><p class="subtitle">Every job that ran in this space: prints, labels and burns.</p></div></div>
<form class="j-filters" data-filters>
  <label>Machine <select name="device_id"><option value="">All</option>{options}</select></label>
  <label>Kind <select name="kind"><option value="">All</option><option value="print">Print</option><option value="label">Label</option><option value="burn">Burn</option></select></label>
  <label>Ended <select name="outcome"><option value="">Any way</option><option value="finished">Done</option><option value="cancelled">Stopped</option><option value="failed">Failed</option></select></label>
  <label>By <input name="person" placeholder="Name" autocomplete="off"></label>
  <label>From <input type="date" name="from"></label>
  <label>To <input type="date" name="to"></label>
  <button type="button" class="btn ghost small" data-clear hidden>Clear filters</button>
</form>
<div class="j-list" data-list><p class="empty">Loading…</p></div>
<p class="j-more"><button class="btn" data-older hidden>Older</button></p>
</main>
<script>
{JOBS_SCRIPT}
</script>
</body>
</html>"##,
        header = header(&space, "", "jobs"),
    ))
    .into_response()
}

const JOBS_STYLE: &str = r#"
.j-filters { display: flex; flex-wrap: wrap; gap: .5rem 1rem; align-items: end; margin: 0 0 1rem; }
.j-filters label { display: flex; flex-direction: column; font-size: .85rem; gap: .2rem; }
.j-table { width: 100%; border-collapse: collapse; }
.j-table th, .j-table td { text-align: left; padding: .45rem .5rem; border-bottom: 1px solid var(--line, #ddd); }
.j-table progress { width: 6rem; }
.j-more { text-align: center; }
@media (max-width: 640px) { .j-table .j-opt { display: none; } }
"#;

const JOBS_SCRIPT: &str = r#"
const form = document.querySelector('[data-filters]');
const list = document.querySelector('[data-list]');
const older = document.querySelector('[data-older]');
const clear = document.querySelector('[data-clear]');
const names = Object.fromEntries([...form.device_id.options].map(o => [o.value, o.textContent]));
const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
const when = t => new Date(t).toLocaleString([], { dateStyle: 'short', timeStyle: 'short' });
const took = (a, b) => { const s = Math.round((new Date(b) - new Date(a)) / 1000); const h = Math.floor(s / 3600), m = Math.floor(s % 3600 / 60); return h ? `${h} h ${m} min` : `${m} min`; };
const ended = { finished: 'Done', cancelled: 'Stopped', failed: 'Failed' };
let cursor = '';
function params() {
  const p = new URLSearchParams();
  for (const [k, v] of new FormData(form)) {
    if (!v) continue;
    if (k === 'from') p.set(k, new Date(v + 'T00:00:00').toISOString());
    else if (k === 'to') p.set(k, new Date(v + 'T23:59:59').toISOString());
    else p.set(k, v);
  }
  return p;
}
function row(j) {
  const file = j.kind === 'print' ? `<a href="files#file=${encodeURIComponent(j.file)}">${esc(j.display_name)}</a>` : esc(j.display_name);
  const state = j.ended ? (ended[j.outcome] ?? esc(j.state)) : esc(j.state);
  const time = j.ended ? took(j.started, j.ended) : `<progress max="100" value="${j.progress}"></progress> ${j.progress}%`;
  return `<tr><td>${file}</td><td><a href="machines/${encodeURIComponent(j.device_id)}">${esc(names[j.device_id] ?? j.device_id)}</a></td><td>${state}</td><td>${when(j.started)}</td><td>${time}</td><td class="j-opt">${esc(j.by)}</td><td class="j-opt">${esc(j.kind)}</td></tr>`;
}
async function load(more) {
  const p = params();
  if (more && cursor) p.set('cursor', cursor);
  const r = await fetch('api/v1/jobs?' + p);
  if (!r.ok) { list.innerHTML = `<p class="empty">Could not read the jobs: ${esc((await r.json().catch(() => ({}))).text ?? r.status)}</p>`; older.hidden = true; return; }
  const page = await r.json();
  cursor = page.cursor;
  older.hidden = !cursor;
  const filtered = [...p.keys()].some(k => k !== 'cursor');
  clear.hidden = !filtered;
  if (!more && !page.items.length) {
    list.innerHTML = filtered ? '<p class="empty">No jobs match these filters.</p>' : '<p class="empty">No jobs yet.</p>';
    return;
  }
  const rows = page.items.map(row).join('');
  if (more) list.querySelector('tbody').insertAdjacentHTML('beforeend', rows);
  else list.innerHTML = `<table class="j-table"><thead><tr><th>File</th><th>Machine</th><th>State</th><th>Started</th><th>Took</th><th class="j-opt">By</th><th class="j-opt">Kind</th></tr></thead><tbody>${rows}</tbody></table>`;
}
form.addEventListener('change', () => load(false));
form.person.addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); load(false); } });
clear.addEventListener('click', () => { form.reset(); load(false); });
older.addEventListener('click', () => load(true));
load(false);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> ListQuery {
        let text: String = pairs.iter().map(|(k, v)| format!("{k}={v}&")).collect();
        let uri: axum::http::Uri = format!("/api/v1/jobs?{text}").parse().unwrap();
        Query::<ListQuery>::try_from_uri(&uri).unwrap().0
    }

    /// Connect's parameters and refusals (`jobs.go:66-81`, `:113-120`), its page size and cap
    /// (`devicejobs.go:652-653`); ours on top.
    #[test]
    fn the_query_is_connects_with_ours() {
        let all = query_of(&q(&[])).unwrap();
        assert_eq!(all.limit, 50);
        assert_eq!((all.ended, all.after), (None, None));
        assert_eq!(query_of(&q(&[("limit", "500")])).unwrap().limit, 200);
        assert_eq!(query_of(&q(&[("limit", "0")])).unwrap().limit, 50);
        assert_eq!(
            query_of(&q(&[("phase", "running")])).unwrap().ended,
            Some(false)
        );
        let ended = query_of(&q(&[
            ("phase", "ended"),
            ("from", "2026-11-27T00:00:00Z"),
            ("to", "2026-11-27T01:00:00%2B01:00"),
            ("cursor", "1795737600.42"),
        ]))
        .unwrap();
        assert_eq!(ended.ended, Some(true));
        assert_eq!(ended.from, Some(1_795_737_600));
        assert_eq!(ended.to, Some(1_795_737_600));
        assert_eq!(ended.after, Some((1_795_737_600, 42)));
        for refused in [
            &[("phase", "done")][..],
            &[("phase", "running"), ("from", "2026-11-27T00:00:00Z")],
            &[
                ("from", "2026-11-28T00:00:00Z"),
                ("to", "2026-11-27T00:00:00Z"),
            ],
            &[("from", "yesterday")],
            &[("cursor", "x")],
            &[("outcome", "done")],
            &[("kind", "cnc")],
        ] {
            assert!(query_of(&q(refused)).is_err(), "{refused:?}");
        }
        let ours = query_of(&q(&[
            ("outcome", "cancelled"),
            ("kind", "label"),
            ("person", "Ada"),
        ]))
        .unwrap();
        assert_eq!(ours.states, Some(vec!["stopped".to_owned()]));
        assert_eq!(ours.kinds, Some(vec![r#""label""#.to_owned()]));
        assert_eq!(ours.person.as_deref(), Some("Ada"));
        assert_eq!(
            query_of(&q(&[("outcome", "unknown")])).unwrap().states,
            Some(vec![])
        );
    }

    /// A row is Connect's `DeviceJob` (`httpapi/devicejobs.go:57-75`) with ours; a label's has no
    /// printer reading, an ended one its `ended` and `outcome`, a running one neither.
    #[test]
    fn a_row_is_connects_device_job_with_ours() {
        let job = Job {
            id: 7,
            machine_id: "m1".into(),
            file_id: "f1".into(),
            file_name: "name.png".into(),
            state: JobState::Finished,
            created_at: 1_795_737_000,
            started_at: Some(1_795_737_600),
            ended_at: Some(1_795_737_660),
            reason: None,
            from_line: None,
            restart_line: None,
            by: Some("Ada".into()),
        };
        assert_eq!(
            row(&job, MachineKind::Label, None),
            json!({"id": "7", "device_job_id": "7", "device_id": "m1", "path": "name.png",
                   "display_name": "name.png", "state": "finished", "progress": 100.0,
                   "time_remaining_ms": 0, "started": "2026-11-27T00:00:00Z",
                   "ended": "2026-11-27T00:01:00Z", "outcome": "finished", "kind": "label",
                   "by": "Ada", "file": "f1"})
        );
        let running = Job {
            state: JobState::Printing,
            ended_at: None,
            ..job
        };
        let live = maker_connect_protocol::JobProgress {
            file: "name.gcode".into(),
            done: 50,
            total: 200,
            elapsed_s: None,
            remaining_s: Some(90),
        };
        let row = row(&running, MachineKind::Fdm, Some(&live));
        assert_eq!(
            (row["progress"].clone(), row["time_remaining_ms"].clone()),
            (json!(25.0), json!(90_000))
        );
        assert_eq!(row["kind"], "print");
        assert!(row.get("ended").is_none() && row.get("outcome").is_none());
    }
}
