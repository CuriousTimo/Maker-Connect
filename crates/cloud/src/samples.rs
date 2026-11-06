//! Example files in the binary: 3DBenchy (CC0 since 2025) sliced for a few printers, to try
//! the website and the virtual printers with real slicer output, thumbnails and times.
//! Packed by `scripts/pack-samples.sh` into one zstd stream (about 3 MB for 18 MB of
//! G-code: the same boat for two printers shares most of its lines). Every space gets them
//! once, so a new user sees at once how it works ([`run`]); the Files page adds them again.

use crate::Shared;
use crate::access::Access;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use std::io::Read;

/// The files, packed: "MCSAMPLES1\n", "<name>\t<bytes>\n" per file, an empty line, the files.
const PACKED: &[u8] = include_bytes!("../samples/samples.zst");

/// Its window covers all the files together (`--long=25`); ours, so we trust it.
const WINDOW: u64 = 64 << 20;

/// The example files: (name, contents).
pub fn unpack() -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut raw = Vec::new();
    ruzstd::decoding::StreamingDecoder::new_with_max_window_size(PACKED, WINDOW)
        .map_err(std::io::Error::other)?
        .read_to_end(&mut raw)?;
    let bad = || std::io::Error::other("not a samples pack");
    let header_end = raw.windows(2).position(|w| w == b"\n\n").ok_or_else(bad)?;
    let header = std::str::from_utf8(&raw[..header_end]).map_err(|_| bad())?;
    let mut lines = header.lines();
    if lines.next() != Some("MCSAMPLES1") {
        return Err(bad());
    }
    let mut at = header_end + 2;
    let mut files = Vec::new();
    for line in lines {
        let (name, size) = line.split_once('\t').ok_or_else(bad)?;
        let size: usize = size.parse().map_err(|_| bad())?;
        let bytes = raw.get(at..at + size).ok_or_else(bad)?.to_vec();
        files.push((name.to_owned(), bytes));
        at += size;
    }
    Ok(files)
}

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/files/examples", post(add))
}

/// Keeps one example file in a space's Files, as an upload would: its thumbnail, what it
/// says of itself.
fn keep(store: &Shared, space: i64, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let id = crate::store::random_id();
    let (path, thumbnail) = {
        let store = store.lock().unwrap();
        (store.file_path(&id), store.thumbnail_path(&id))
    };
    std::fs::write(&path, bytes)?;
    let (meta, png) = crate::gcode::read(&path).unwrap_or_default();
    if let Some(png) = png {
        let _ = std::fs::write(&thumbnail, png);
    }
    store
        .lock()
        .unwrap()
        .add_file(space, &id, name, bytes.len() as u64, &meta)
        .map_err(std::io::Error::other)
}

/// Puts the example files a space does not have by name in its Files: how many.
fn add_to(store: &Shared, space: i64, files: &[(String, Vec<u8>)]) -> std::io::Result<usize> {
    let have: Vec<String> = store
        .lock()
        .unwrap()
        .files(space)
        .map_err(std::io::Error::other)?
        .into_iter()
        .map(|f| f.name)
        .collect();
    let mut added = 0;
    for (name, bytes) in files {
        if !have.contains(name) {
            keep(store, space, name, bytes)?;
            added += 1;
        }
    }
    Ok(added)
}

/// Every space gets the example files once, so whoever starts sees how it works: a new
/// space within moments, the ones there already at the next start. Removed, they stay
/// removed (the Files page can add them again).
pub fn give_once(store: &Shared) -> std::io::Result<()> {
    let pending: Vec<i64> = {
        let store = store.lock().unwrap();
        let mut q = store
            .db
            .prepare("SELECT id FROM spaces WHERE samples_given = 0")
            .map_err(std::io::Error::other)?;
        q.query_map([], |r| r.get(0))
            .map_err(std::io::Error::other)?
            .collect::<rusqlite::Result<_>>()
            .map_err(std::io::Error::other)?
    };
    if pending.is_empty() {
        return Ok(());
    }
    let files = unpack()?;
    for space in pending {
        let added = add_to(store, space, &files)?;
        store
            .lock()
            .unwrap()
            .db
            .execute("UPDATE spaces SET samples_given = 1 WHERE id = ?1", [space])
            .map_err(std::io::Error::other)?;
        if added > 0 {
            println!("example files: {added} into space {space}");
        }
    }
    Ok(())
}

/// Now and then: spaces that have not had the example files yet get them.
pub async fn run(store: Shared) {
    loop {
        let s = store.clone();
        if let Ok(Err(error)) = tokio::task::spawn_blocking(move || give_once(&s)).await {
            eprintln!("example files: {error}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

/// The Files page's button: the example files not there by name, again.
async fn add(access: Access, State(store): State<Shared>) -> Response {
    let space = access.space;
    let added = tokio::task::spawn_blocking(move || add_to(&store, space, &unpack()?)).await;
    match added {
        Ok(Ok(added)) => Json(serde_json::json!({ "added": added })).into_response(),
        Ok(Err(error)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("not added: {error}"),
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "not added").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    /// A space gets them once: not twice, and not again after they were removed.
    #[test]
    fn every_space_gets_them_once() {
        let store: crate::Shared =
            Arc::new(Mutex::new(crate::store::Store::open(":memory:").unwrap()));
        let space = store.lock().unwrap().default_space().unwrap().0;
        super::give_once(&store).unwrap();
        let n = store.lock().unwrap().files(space).unwrap().len();
        assert_eq!(n, super::unpack().unwrap().len());
        let first = store.lock().unwrap().files(space).unwrap()[0].clone();
        store.lock().unwrap().remove_file(space, &first.id).unwrap();
        super::give_once(&store).unwrap();
        assert_eq!(
            store.lock().unwrap().files(space).unwrap().len(),
            n - 1,
            "removed stays removed"
        );
        let dir = store.lock().unwrap().files_dir.clone();
        std::fs::remove_dir_all(dir).ok();
    }

    /// The pack unpacks into what it was made of: the Benchies, their times and thumbnails.
    #[test]
    fn the_examples_unpack() {
        let files = super::unpack().unwrap();
        assert!(files.len() >= 2, "{} files", files.len());
        let total: usize = files.iter().map(|(_, b)| b.len()).sum();
        assert!(
            total > 4 * crate::samples::PACKED.len(),
            "packed well: {total} bytes"
        );
        let dir = std::env::temp_dir().join(format!("mc-samples-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in &files {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            let (meta, _) = crate::gcode::read(&path).unwrap();
            assert!(meta.estimated_s.is_some(), "{name}: its time");
            assert!(
                meta.filament_g.is_some() || meta.filament_m.is_some(),
                "{name}: its filament"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
