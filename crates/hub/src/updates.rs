//! The hub's updates from the project's releases on GitHub, checked against Maker Connect's
//! release key (crate `maker-connect-update`), from About → Update (who runs the server only).
//! The newest `hub-v…` release for this system is fetched and checked, and the new build tried.
//! Then it takes the old one's place and starts; if it does not stay up, the old build comes
//! back (`main`).

use crate::access::Access;
use crate::{Store, failed};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use maker_connect_update as update;
use serde::Deserialize;

pub fn routes() -> Router<Store> {
    Router::new()
        .route("/api/update", get(hub_status))
        .route("/about/update", post(update_hub))
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

/// The newest release of `part` (`hub`), betas too.
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

/// `hub-v0.3.0` → `v0.3.0`.
fn version_of(tag: &str) -> &str {
    tag.split_once('-').map_or(tag, |(_, v)| v)
}

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
async fn hub_status(State(store): State<Store>) -> Response {
    let printing: i64 = store.printing_anywhere().await.unwrap_or(0);
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
/// `exe`. Returns the release it put in place.
///
/// With `dumps`, the database is dumped first (kept as the way back, in the trial's note), and the
/// new build is tried on a copy of it, restored from that dump (crate::backup).
pub(crate) fn prepare_hub(
    exe: &std::path::Path,
    url: &str,
    dumps: Option<&crate::backup::Dumps>,
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
    // The new build tried with folders of its own.
    let trial_dir = std::env::temp_dir().join(format!("maker-connect-trial-{}", update::now()));
    std::fs::create_dir_all(&trial_dir).map_err(|e| e.to_string())?;
    let trial_db = trial_dir.join("maker-connect.db");
    let mut args: Vec<std::ffi::OsString> =
        vec!["--trial".into(), "--db".into(), trial_db.into_os_string()];
    let mut note = serde_json::json!({});
    if let Some(dumps) = dumps {
        let dump = dumps
            .dump(crate::backup::BEFORE_UPDATE, Some(&tag))
            .map_err(|why| format!("the database could not be backed up: {why}"))?;
        let copy = dumps.trial_database();
        dumps
            .restore_as(&dump, &copy)
            .map_err(|why| format!("the copy to try the new build on failed: {why}"))?;
        args.extend(["--trial-database".into(), copy.into()]);
        note = serde_json::json!({ "dump": dump });
    }
    let installed = update::install(
        exe,
        &build,
        &args,
        crate::store::BUILD,
        version_of(&tag),
        120,
        note,
    );
    let _ = std::fs::remove_dir_all(&trial_dir);
    if let Some(dumps) = dumps {
        let _ = dumps.drop_database(&dumps.trial_database());
    }
    installed.map(|()| tag)
}

/// Told when a new build is in place: the hub stops serving and tries it (main).
pub static UPDATE: tokio::sync::Notify = tokio::sync::Notify::const_new();

async fn update_hub(access: Access, State(store): State<Store>) -> Response {
    let Ok(exe) = std::env::current_exe() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "where this program is, is not known",
        )
            .into_response();
    };
    let program = exe.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_hub(&program, &releases_url(), crate::backup::DUMPS.get())
    })
    .await;
    match prepared {
        Ok(Ok(tag)) => {
            let _ = store
                .audit(
                    &access,
                    "updated the hub",
                    &tag,
                    &format!("from {}", crate::store::BUILD),
                )
                .await;
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

    #[cfg(unix)]
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

    /// The hub fetches its newest release, checks it and tries the new build before it
    /// takes the old one's place; a tampered archive or one that fails its trial changes
    /// nothing.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_hub_updates_itself_only_to_a_signed_build_that_works() {
        let dir = std::env::temp_dir().join(format!("mc-hub-update-{}", crate::store::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("maker-connect-hub");
        std::fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        let system = update::this_system().unwrap();
        let name: &'static str =
            Box::leak(format!("maker-connect-hub-{system}.tar.gz").into_boxed_str());
        // The trial is given folders of its own.
        let works = b"#!/bin/sh\n[ \"$1\" = --trial ] && [ \"$2\" = --db ]\n".to_vec();
        let url = releases(vec![("hub-v99.0.0", name, archive(&works))]).await;
        let (e, u) = (exe.clone(), url.clone());
        let tag = tokio::task::spawn_blocking(move || prepare_hub(&e, &u, None))
            .await
            .unwrap();
        assert_eq!(tag, Ok("hub-v99.0.0".to_owned()));
        assert_eq!(std::fs::read(&exe).unwrap(), works);
        assert!(update::previous(&exe).is_file());
        let Start::OnTrial(trial) = update::on_start(&exe) else {
            panic!("on trial")
        };
        update::confirm(&exe, &trial);

        // A build that fails its trial is not put in place.
        std::fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        let fails = b"#!/bin/sh\nexit 1\n".to_vec();
        let url = releases(vec![("hub-v99.0.1", name, archive(&fails))]).await;
        let e = exe.clone();
        let error = tokio::task::spawn_blocking(move || prepare_hub(&e, &url, None))
            .await
            .unwrap();
        assert!(error.unwrap_err().contains("trial"));
        assert_eq!(
            std::fs::read_to_string(&exe).unwrap(),
            "#!/bin/sh\necho old\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    use maker_connect_update::Start;

    /// Before a self-update the database is dumped and the new build tried on a copy restored from
    /// that dump; when the new build then does not stay up, the way back brings the database back as
    /// the dump holds it (main's `go_back`), together with the build before.
    #[cfg(unix)]
    #[sqlx::test(migrator = "crate::store::pg::MIGRATOR")]
    #[cfg_attr(
        not(feature = "pg-tests"),
        ignore = "needs Postgres: the pg-tests feature"
    )]
    async fn an_update_is_backed_up_tried_on_a_copy_and_goes_back_with_its_database(
        pool: sqlx::PgPool,
    ) {
        use crate::backup;
        // A database of its own with a short name (the test's own is too long for `_back`).
        let tools = backup::for_test(&pool).await;
        let name = format!("mc_live_{}", crate::store::random_id());
        tools
            .restore_as(&tools.dump(backup::NIGHTLY, None).unwrap(), &name)
            .unwrap();
        let base = std::env::var("DATABASE_URL").unwrap();
        let live_url = backup::with_database(&base, &name).unwrap();
        let live = sqlx::PgPool::connect(&live_url).await.unwrap();
        sqlx::query("INSERT INTO meta (key, value) VALUES ('before', 'update')")
            .execute(&live)
            .await
            .unwrap();
        let dumps = backup::Dumps::new(
            tools.dir.join("live"),
            backup::KEEP,
            &std::env::var("MC_PG_COMMAND").unwrap_or_default(),
            &backup::hub_url_for_test(&name),
        )
        .unwrap();

        let dir = std::env::temp_dir().join(format!("mc-hub-update-{}", crate::store::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("maker-connect-hub");
        std::fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        let system = update::this_system().unwrap();
        let file: &'static str =
            Box::leak(format!("maker-connect-hub-{system}.tar.gz").into_boxed_str());
        // The trial is told the copy to run on.
        let works = format!(
            "#!/bin/sh\n[ \"$1\" = --trial ] && [ \"$4\" = --trial-database ] && [ \"$5\" = {name}_trial ]\n"
        )
        .into_bytes();
        let url = releases(vec![("hub-v99.0.0", file, archive(&works))]).await;
        let (e, d) = (exe.clone(), dumps.clone());
        let tag = tokio::task::spawn_blocking(move || prepare_hub(&e, &url, Some(&d)))
            .await
            .unwrap();
        assert_eq!(tag, Ok("hub-v99.0.0".to_owned()));
        assert_eq!(std::fs::read(&exe).unwrap(), works);
        // Backed up; the copy is gone again.
        let backed_up = dumps.of_kind(backup::BEFORE_UPDATE);
        assert_eq!(backed_up.len(), 1);
        let copies: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_database WHERE datname = $1")
            .bind(format!("{name}_trial"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(copies, 0);

        // The new build changes the database, then does not stay up: the way back.
        sqlx::query("CREATE TABLE only_in_the_new_build (x int)")
            .execute(&live)
            .await
            .unwrap();
        sqlx::query("UPDATE meta SET value = 'changed' WHERE key = 'before'")
            .execute(&live)
            .await
            .unwrap();
        live.close().await;
        let Start::OnTrial(trial) = update::on_start(&exe) else {
            panic!("on trial")
        };
        let d = dumps.clone();
        let back = tokio::task::spawn_blocking(move || backup::back_as_before(&d, &trial.note))
            .await
            .unwrap();
        assert_eq!(back, Ok(Some(backed_up[0].clone())));
        let live = sqlx::PgPool::connect(&live_url).await.unwrap();
        let value: String = sqlx::query_scalar("SELECT value FROM meta WHERE key = 'before'")
            .fetch_one(&live)
            .await
            .unwrap();
        assert_eq!(value, "update");
        let new_table: Option<String> =
            sqlx::query_scalar("SELECT to_regclass('only_in_the_new_build')::text")
                .fetch_one(&live)
                .await
                .unwrap();
        assert_eq!(new_table, None);
        live.close().await;
        tools.drop_database(&name).unwrap();
        std::fs::remove_dir_all(&tools.dir).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
