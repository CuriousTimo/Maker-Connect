//! Updates from the project's releases on GitHub, checked against Maker Connect's release
//! key (crate `maker-connect-update`):
//!
//! - **The hub itself** (About → Update; who runs the server only). The newest `hub-v…`
//!   release for this system is fetched and checked, the database backed up, and the new
//!   build tried on a copy of the database. Then it takes the old one's place and starts; if
//!   it does not stay up, the old build and the backup come back (`main`).
//! - **Agents** (a machine's Connection card, or every agent of a space; its admins). The
//!   newest `agent-v…` release's signed list is fetched and kept; agents are told in their
//!   reports, fetch their build through the hub (agents need no internet), check it
//!   themselves, and update once their machine is idle, with their own way back.

use crate::access::Access;
use crate::{Shared, failed};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use maker_connect_update as update;
use serde::Deserialize;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/update", get(hub_status))
        .route("/about/update", post(update_hub))
        .route("/machines/{id}/agent/update", post(update_agent))
        .route("/agents/update", post(update_agents))
        .route("/api/agent/v1/update/{tag}/{file}", get(agent_build))
}

/// The key releases are checked with: Maker Connect's, or in tests one of their own.
fn key() -> Vec<u8> {
    #[cfg(test)]
    {
        tests::KEY.public.clone()
    }
    #[cfg(not(test))]
    {
        update::RELEASE_KEY.to_vec()
    }
}

/// Where the releases are listed: GitHub, or (for a mirror, or a test) another place that
/// answers the same way. The key is the same either way.
fn releases_url() -> String {
    std::env::var("MAKER_CONNECT_RELEASES").unwrap_or_else(|_| update::RELEASES.to_owned())
}

#[derive(Debug, Clone, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    fn asset(&self, name: &str) -> Result<&Asset, String> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("{} has no {name}", self.tag_name))
    }
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("user-agent", "maker-connect-hub")
        .call()
        .map_err(|e| format!("could not fetch {url}: {e}"))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(response.into_reader(), 256 << 20),
        &mut bytes,
    )
    .map_err(|e| format!("could not fetch {url}: {e}"))?;
    Ok(bytes)
}

/// The newest release of `part` (`hub`, `agent`), betas too.
fn newest(url: &str, part: &str) -> Result<Release, String> {
    let releases: Vec<Release> = serde_json::from_slice(&fetch(url)?)
        .map_err(|e| format!("the list of releases does not read: {e}"))?;
    releases
        .into_iter()
        .find(|r| !r.draft && r.tag_name.starts_with(&format!("{part}-v")))
        .ok_or_else(|| format!("no {part} release yet"))
}

/// A release's list of sums and its signature, checked.
fn signed(release: &Release) -> Result<(String, String), String> {
    let text = |name: &str| -> Result<String, String> {
        let bytes = fetch(&release.asset(name)?.browser_download_url)?;
        String::from_utf8(bytes).map_err(|_| format!("{name} is not text"))
    };
    let sums = text("SHA256SUMS")?;
    let signature = text("SHA256SUMS.sig")?;
    update::verify(&release.tag_name, &sums, &signature, &key())?;
    Ok((sums, signature))
}

/// `agent-v0.3.0` → `v0.3.0`.
fn version_of(tag: &str) -> &str {
    tag.split_once('-').map_or(tag, |(_, v)| v)
}

// The hub.

/// The hub's own binary out of its release archive (`maker-connect-hub-<system>.tar.gz`).
fn unpack(archive: &[u8], program: &str) -> Result<Vec<u8>, String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if path.file_name().is_some_and(|n| n == program) {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|e| e.to_string())?;
            return Ok(bytes);
        }
    }
    Err(format!("the archive has no {program}"))
}

/// What the About page says of updating the hub.
async fn hub_status(State(store): State<Shared>) -> Response {
    let printing: i64 = store
        .lock()
        .unwrap()
        .db
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('printing', 'paused')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let found = tokio::task::spawn_blocking(|| newest(&releases_url(), "hub")).await;
    let last = std::env::current_exe()
        .ok()
        .and_then(|exe| update::last(&exe));
    let mut answer = serde_json::json!({
        "current": crate::store::BUILD,
        "printing": printing,
        "last": last.map(|o| serde_json::json!({
            "at": o.at, "from": o.from, "to": o.to, "ok": o.ok, "note": o.note
        })),
        "windows": cfg!(windows),
    });
    match found {
        Ok(Ok(release)) => {
            answer["newest"] = release.tag_name.clone().into();
            answer["newer"] = update::newer(&release.tag_name, crate::store::BUILD).into();
            answer["notes"] = release.body.unwrap_or_default().into();
        }
        // Not a fault: nothing released yet.
        Ok(Err(error)) if error.starts_with("no ") => answer["none"] = true.into(),
        Ok(Err(error)) => answer["error"] = error.into(),
        Err(error) => answer["error"] = error.to_string().into(),
    }
    Json(answer).into_response()
}

/// Fetches, checks and tries the newest hub release, and puts it in place of the program at
/// `exe`; the database backed up first. Returns the release it put in place.
pub(crate) fn prepare_hub(
    store: &Shared,
    exe: &std::path::Path,
    url: &str,
) -> Result<String, String> {
    let release = newest(url, "hub")?;
    let tag = release.tag_name.clone();
    if !update::newer(&tag, crate::store::BUILD) {
        return Err(format!(
            "this hub is {}, the newest release",
            crate::store::BUILD
        ));
    }
    if cfg!(windows) {
        return Err(
            "on Windows, the hub is updated by hand: unpack the new release over this one".into(),
        );
    }
    let system = update::this_system().ok_or("no release is built for this system")?;
    let name = format!("maker-connect-hub-{system}.tar.gz");
    let (sums, signature) = signed(&release)?;
    let archive = fetch(&release.asset(&name)?.browser_download_url)?;
    update::check(&archive, &name, &tag, &sums, &signature, &key())?;
    let build = unpack(&archive, "maker-connect-hub")?;
    // The database as it is now: kept, for the way back; and a copy to try the new build on.
    let (db, backup, trial_dir) = {
        let store = store.lock().unwrap();
        let file = store
            .db
            .path()
            .filter(|p| !p.is_empty())
            .ok_or("the database is not a file")?;
        let db = std::fs::canonicalize(file).map_err(|e| format!("{file}: {e}"))?;
        let backups = db.with_file_name("maker-connect-backups");
        std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
        let backup = backups.join(format!("maker-connect-{}-before-{tag}.db", update::now()));
        let trial_dir = std::env::temp_dir().join(format!("maker-connect-trial-{}", update::now()));
        std::fs::create_dir_all(&trial_dir).map_err(|e| e.to_string())?;
        for to in [&backup, &trial_dir.join("maker-connect.db")] {
            store
                .db
                .execute("VACUUM INTO ?1", [to.to_string_lossy()])
                .map_err(|e| format!("could not copy the database: {e}"))?;
        }
        (db, backup, trial_dir)
    };
    let trial_db = trial_dir.join("maker-connect.db");
    let installed = update::install(
        exe,
        &build,
        &["--trial".into(), "--db".into(), trial_db.into_os_string()],
        crate::store::BUILD,
        version_of(&tag),
        120,
        serde_json::json!({ "db": db, "backup": backup }),
    );
    let _ = std::fs::remove_dir_all(&trial_dir);
    installed.map(|()| tag)
}

/// Told when a new build is in place: the hub stops serving and tries it (main).
pub(crate) static UPDATE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The database back as it was before an update, for the build before.
pub(crate) fn restore_database(trial: &update::Trial) {
    let (Some(db), Some(backup)) = (trial.note["db"].as_str(), trial.note["backup"].as_str())
    else {
        return;
    };
    if std::fs::copy(backup, db).is_ok() {
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{db}{suffix}"));
        }
        println!("database back as it was before the update ({backup})");
    }
}

async fn update_hub(access: Access, State(store): State<Shared>) -> Response {
    let Ok(exe) = std::env::current_exe() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "where this program is, is not known",
        )
            .into_response();
    };
    let shared = store.clone();
    let program = exe.clone();
    let prepared =
        tokio::task::spawn_blocking(move || prepare_hub(&shared, &program, &releases_url())).await;
    match prepared {
        Ok(Ok(tag)) => {
            let _ = store.lock().unwrap().audit(
                &access,
                "updated the hub",
                &tag,
                &format!("from {}", crate::store::BUILD),
            );
            println!("updating to {tag}: trying it");
            // The answer first; then this hub stops serving, and tries the new build (main).
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                UPDATE.notify_one();
            });
            Json(serde_json::json!({ "restarting": tag })).into_response()
        }
        Ok(Err(why)) => (StatusCode::CONFLICT, why).into_response(),
        Err(error) => failed(error),
    }
}

// Agents.

/// An agent release, signed list checked, as the hub keeps it to pass on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prepared {
    pub tag: String,
    pub sums: String,
    pub signature: String,
}

fn kept_dir(downloads: &std::path::Path, tag: &str) -> Option<std::path::PathBuf> {
    // A tag as GitHub has it: no way out of the folder.
    let plain = tag.starts_with("agent-v")
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c));
    plain.then(|| downloads.join(".updates").join(tag))
}

/// The newest agent release, its signed list fetched, checked and kept.
fn prepare_agent(downloads: &std::path::Path, url: &str) -> Result<Prepared, String> {
    let release = newest(url, "agent")?;
    let (sums, signature) = signed(&release)?;
    let dir = kept_dir(downloads, &release.tag_name).ok_or("an odd release tag")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("SHA256SUMS"), &sums).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("SHA256SUMS.sig"), &signature).map_err(|e| e.to_string())?;
    Ok(Prepared {
        tag: release.tag_name,
        sums,
        signature,
    })
}

/// An agent release kept, its list checked again.
fn kept(downloads: &std::path::Path, tag: &str) -> Option<Prepared> {
    let dir = kept_dir(downloads, tag)?;
    let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).ok()?;
    let signature = std::fs::read_to_string(dir.join("SHA256SUMS.sig")).ok()?;
    update::verify(tag, &sums, &signature, &key()).ok()?;
    Some(Prepared {
        tag: tag.to_owned(),
        sums,
        signature,
    })
}

/// The file an agent of `os` and `arch` needs from a release.
fn agent_file(os: &str, arch: &str) -> Option<String> {
    let exe = if os == "windows" { ".exe" } else { "" };
    update::system(os, arch).map(|system| format!("maker-connect-agent-{system}{exe}"))
}

/// What to tell an agent in its report's answer: the update asked for it, until it is on
/// that release or says how its update went.
pub(crate) fn order_for(
    store: &crate::store::Store,
    token: &str,
    report: &maker_connect_protocol::Telemetry,
) -> rusqlite::Result<Option<maker_connect_protocol::AgentUpdate>> {
    let asked: Option<(String, Option<String>, i64)> = store
        .db
        .query_row(
            "SELECT id, agent_update_to, COALESCE(agent_update_at, 0) FROM machines WHERE token = ?1",
            [token],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((machine, Some(tag), at)) = asked else {
        return Ok(None);
    };
    let Some(agent) = &report.agent else {
        return Ok(None);
    };
    let version = version_of(&tag);
    let answered = agent
        .last_update
        .as_ref()
        .is_some_and(|o| o.to == version && o.at >= at);
    // On that release or a newer one already, or it said how it went: asked no more.
    if !update::newer(version, &agent.version) && update::Version::parse(&agent.version).is_some()
        || answered
    {
        store.db.execute(
            "UPDATE machines SET agent_update_to = NULL, agent_update_at = NULL WHERE id = ?1",
            [&machine],
        )?;
        return Ok(None);
    }
    if !agent.can_update {
        return Ok(None);
    }
    let (Some(prepared), Some(file)) = (
        kept(&store.downloads_dir, &tag),
        agent_file(&agent.os, &agent.arch),
    ) else {
        return Ok(None);
    };
    Ok(Some(maker_connect_protocol::AgentUpdate {
        version: version.to_owned(),
        download: format!("/api/agent/v1/update/{tag}/{file}"),
        file,
        tag: prepared.tag,
        sums: prepared.sums,
        signature: prepared.signature,
    }))
}

/// Asks the agents of `machines` (in space `space`) to update to `tag`.
fn ask(
    store: &crate::store::Store,
    space: i64,
    machines: &[String],
    tag: &str,
) -> rusqlite::Result<usize> {
    let mut asked = 0;
    for machine in machines {
        asked += store.db.execute(
            "UPDATE machines SET agent_update_to = ?1, agent_update_at = ?2
             WHERE id = ?3 AND space_id = ?4 AND link = 'agent'",
            rusqlite::params![tag, update::now(), machine, space],
        )?;
    }
    Ok(asked)
}

async fn prepared(store: &Shared) -> Result<Prepared, Box<Response>> {
    let downloads = store.lock().unwrap().downloads_dir.clone();
    match tokio::task::spawn_blocking(move || prepare_agent(&downloads, &releases_url())).await {
        Ok(Ok(p)) => Ok(p),
        Ok(Err(why)) => Err(Box::new((StatusCode::CONFLICT, why).into_response())),
        Err(error) => Err(Box::new(failed(error))),
    }
}

async fn update_agent(
    access: Access,
    State(store): State<Shared>,
    Path(id): Path<String>,
) -> Response {
    let prepared = match prepared(&store).await {
        Ok(p) => p,
        Err(response) => return *response,
    };
    let store = store.lock().unwrap();
    match ask(
        &store,
        access.space,
        std::slice::from_ref(&id),
        &prepared.tag,
    ) {
        Ok(0) => (
            StatusCode::CONFLICT,
            "only a machine with Maker Connect's agent updates it",
        )
            .into_response(),
        Ok(_) => {
            let _ = store.audit(&access, "asked an agent to update", &id, &prepared.tag);
            Json(serde_json::json!({ "to": prepared.tag })).into_response()
        }
        Err(error) => failed(error),
    }
}

async fn update_agents(access: Access, State(store): State<Shared>) -> Response {
    let prepared = match prepared(&store).await {
        Ok(p) => p,
        Err(response) => return *response,
    };
    let store = store.lock().unwrap();
    let machines: Vec<String> = match store.machines(access.space) {
        Ok(machines) => machines
            .into_iter()
            .filter(|m| m.link == "agent")
            .map(|m| m.id)
            .collect(),
        Err(error) => return failed(error),
    };
    match ask(&store, access.space, &machines, &prepared.tag) {
        Ok(asked) => {
            let _ = store.audit(
                &access,
                "asked every agent to update",
                &prepared.tag,
                &format!("{asked} agents"),
            );
            Json(serde_json::json!({ "to": prepared.tag, "agents": asked })).into_response()
        }
        Err(error) => failed(error),
    }
}

/// An agent's build, for an agent asked to update (its token): kept here once fetched, and
/// checked against the release's signed list before it is kept or handed out.
async fn agent_build(
    State(store): State<Shared>,
    headers: HeaderMap,
    Path((tag, file)): Path<(String, String)>,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let (downloads, known) = {
        let store = store.lock().unwrap();
        let known = !token.is_empty() && store.machine_id_by_token(token).ok().flatten().is_some();
        (store.downloads_dir.clone(), known)
    };
    let not_found = || (StatusCode::NOT_FOUND, "no such build").into_response();
    if !known || !file.starts_with("maker-connect-agent-") || file.contains(['/', '\\']) {
        return not_found();
    }
    let Some(prepared) = kept(&downloads, &tag) else {
        return not_found();
    };
    let dir = kept_dir(&downloads, &tag).expect("kept");
    let found = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let path = dir.join(&file);
        let build = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                let releases: Vec<Release> =
                    serde_json::from_slice(&fetch(&releases_url())?).map_err(|e| e.to_string())?;
                let release = releases
                    .iter()
                    .find(|r| r.tag_name == tag)
                    .ok_or("the release is gone")?;
                fetch(&release.asset(&file)?.browser_download_url)?
            }
        };
        update::check(
            &build,
            &file,
            &tag,
            &prepared.sums,
            &prepared.signature,
            &key(),
        )?;
        let _ = std::fs::write(&path, &build);
        Ok(build)
    })
    .await;
    match found {
        Ok(Ok(build)) => ([("content-type", "application/octet-stream")], build).into_response(),
        Ok(Err(why)) => (StatusCode::BAD_GATEWAY, why).into_response(),
        Err(error) => failed(error),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::Engine;
    use ring::signature::KeyPair;
    use std::sync::LazyLock;

    pub(crate) struct TestKey {
        pub pair: ring::signature::Ed25519KeyPair,
        pub public: Vec<u8>,
    }

    /// The tests' release key, in place of Maker Connect's.
    pub(crate) static KEY: LazyLock<TestKey> = LazyLock::new(|| {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = pair.public_key().as_ref().to_vec();
        TestKey { pair, public }
    });

    /// A release as GitHub lists it, served by a small server of the test's own.
    pub(crate) async fn releases(files: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
        // (tag, name, bytes); every tag gets its SHA256SUMS and signature.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut served: std::collections::HashMap<String, Vec<u8>> = Default::default();
        let mut tags: Vec<&str> = files.iter().map(|f| f.0).collect();
        tags.dedup();
        let mut list = Vec::new();
        for tag in tags {
            let mine: Vec<_> = files.iter().filter(|f| f.0 == tag).collect();
            let sums: String = mine
                .iter()
                .map(|f| format!("{}  {}\n", update::sha256_hex(&f.2), f.1))
                .collect();
            let signature = base64::engine::general_purpose::STANDARD
                .encode(KEY.pair.sign(update::signed_text(tag, &sums).as_bytes()));
            let mut assets = Vec::new();
            for (name, bytes) in mine.iter().map(|f| (f.1.to_owned(), f.2.clone())).chain([
                ("SHA256SUMS".to_owned(), sums.into_bytes()),
                ("SHA256SUMS.sig".to_owned(), signature.into_bytes()),
            ]) {
                let path = format!("/{tag}/{name}");
                assets.push(serde_json::json!({ "name": name, "browser_download_url": format!("{base}{path}") }));
                served.insert(path, bytes);
            }
            list.push(serde_json::json!({ "tag_name": tag, "body": "notes", "assets": assets }));
        }
        served.insert("/releases".into(), serde_json::to_vec(&list).unwrap());
        let served = std::sync::Arc::new(served);
        let app = Router::new().fallback(move |uri: axum::http::Uri| {
            let served = served.clone();
            async move {
                match served.get(uri.path()) {
                    Some(bytes) => bytes.clone().into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("{base}/releases")
    }

    fn archive(program: &[u8]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(program.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(
            &mut header,
            "maker-connect-hub-x/maker-connect-hub",
            program,
        )
        .unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar.into_inner().unwrap()).unwrap();
        gz.finish().unwrap()
    }

    /// The hub fetches its newest release, checks it, backs its database up and tries the
    /// new build on a copy before it takes the old one's place; a tampered archive or one
    /// that fails its trial changes nothing.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_hub_updates_itself_only_to_a_signed_build_that_works() {
        let dir = std::env::temp_dir().join(format!("mc-hub-update-{}", crate::store::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("maker-connect.db");
        let store: Shared = std::sync::Arc::new(std::sync::Mutex::new(
            crate::store::Store::open(db.to_str().unwrap()).unwrap(),
        ));
        let exe = dir.join("maker-connect-hub");
        std::fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        let system = update::this_system().unwrap();
        let name: &'static str =
            Box::leak(format!("maker-connect-hub-{system}.tar.gz").into_boxed_str());
        // The trial is given a copy of the database: the new build must find it.
        let works = b"#!/bin/sh\n[ \"$1\" = --trial ] && [ -f \"$3\" ]\n".to_vec();
        let url = releases(vec![("hub-v99.0.0", name, archive(&works))]).await;
        let (s, e, u) = (store.clone(), exe.clone(), url.clone());
        let tag = tokio::task::spawn_blocking(move || prepare_hub(&s, &e, &u))
            .await
            .unwrap();
        assert_eq!(tag, Ok("hub-v99.0.0".to_owned()));
        assert_eq!(std::fs::read(&exe).unwrap(), works);
        assert!(update::previous(&exe).is_file());
        let Start::OnTrial(trial) = update::on_start(&exe) else {
            panic!("on trial")
        };
        let backup = trial.note["backup"].as_str().unwrap().to_owned();
        assert!(
            std::path::Path::new(&backup).is_file(),
            "the database backed up"
        );
        update::confirm(&exe, &trial);

        // A build that fails its trial is not put in place.
        std::fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        let fails = b"#!/bin/sh\nexit 1\n".to_vec();
        let url = releases(vec![("hub-v99.0.1", name, archive(&fails))]).await;
        let (s, e) = (store.clone(), exe.clone());
        let error = tokio::task::spawn_blocking(move || prepare_hub(&s, &e, &url))
            .await
            .unwrap();
        assert!(error.unwrap_err().contains("trial"));
        assert_eq!(
            std::fs::read_to_string(&exe).unwrap(),
            "#!/bin/sh\necho old\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    use maker_connect_update::Start;

    /// An agent asked to update gets the order, fetches its build through the hub (checked),
    /// and is no longer asked once it says how it went.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_agent_is_told_and_fetches_its_build_through_the_hub() {
        let build = b"a new agent".to_vec();
        let url = releases(vec![(
            "agent-v99.0.0",
            "maker-connect-agent-linux-armv6",
            build.clone(),
        )])
        .await;
        let store = crate::store::Store::open(":memory:").unwrap();
        let (machine_id, token, code) = store
            .register(&maker_connect_protocol::Register {
                protocol: maker_connect_protocol::VERSION,
                name: "Pi".into(),
                kind: maker_connect_protocol::MachineKind::Label,
                agent_version: "v0.2.0-beta.2".into(),
            })
            .unwrap();
        let space = 1;
        store.pair(space, &code).unwrap();
        let downloads = store.downloads_dir.clone();
        let u = url.clone();
        let prepared = tokio::task::spawn_blocking(move || prepare_agent(&downloads, &u))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ask(
                &store,
                space,
                std::slice::from_ref(&machine_id),
                &prepared.tag
            )
            .unwrap(),
            1
        );

        let mut report = maker_connect_protocol::Telemetry {
            agent: Some(maker_connect_protocol::AgentInfo {
                version: "v0.2.0-beta.2".into(),
                os: "linux".into(),
                arch: "arm".into(),
                can_update: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let order = order_for(&store, &token, &report).unwrap().unwrap();
        assert_eq!(order.version, "v99.0.0");
        assert_eq!(order.file, "maker-connect-agent-linux-armv6");
        update::check(
            &build,
            &order.file,
            &order.tag,
            &order.sums,
            &order.signature,
            &KEY.public,
        )
        .unwrap();

        // Through the hub, with the agent's token only.
        let shared: Shared = std::sync::Arc::new(std::sync::Mutex::new(store));
        let app = routes().with_state(shared.clone());
        let get = |token: &'static str| {
            let app = app.clone();
            let path = order.download.clone();
            async move {
                use tower::ServiceExt;
                let response = app
                    .oneshot(
                        axum::http::Request::get(path)
                            .header("authorization", format!("Bearer {token}"))
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = response.status();
                let body = http_body_util::BodyExt::collect(response.into_body())
                    .await
                    .unwrap()
                    .to_bytes();
                (status, body.to_vec())
            }
        };
        assert_eq!(get("nobody").await.0, StatusCode::NOT_FOUND);
        let token: &'static str = Box::leak(token.clone().into_boxed_str());
        // The release must be found where the test serves it.
        unsafe { std::env::set_var("MAKER_CONNECT_RELEASES", &url) };
        assert_eq!(get(token).await, (StatusCode::OK, build));

        // An agent that says how it went is not asked again.
        report.agent.as_mut().unwrap().last_update = Some(maker_connect_protocol::UpdateOutcome {
            at: update::now() + 1,
            from: "v0.2.0-beta.2".into(),
            to: "v99.0.0".into(),
            ok: false,
            note: "it did not reach the hub in time".into(),
        });
        let store = shared.lock().unwrap();
        assert_eq!(order_for(&store, token, &report).unwrap(), None);
    }
}
