//! The registry docs (`docs/registries/*.md`) are generated from the tables in
//! `crates/device/src/registry.rs` and must not drift from them (hub-core-v2.md §2.10).
//!
//! Check: `cargo test -p maker-connect-device --test registry_docs` (run by `scripts/check.sh`).
//! Write: `MC_WRITE_REGISTRIES=1 cargo test -p maker-connect-device --test registry_docs`.

use std::path::PathBuf;

#[test]
fn registry_docs_match_the_tables() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/registries");
    let write = std::env::var_os("MC_WRITE_REGISTRIES").is_some();
    let mut drift = Vec::new();
    for (name, text) in maker_connect_device::registry::render_docs() {
        let path = dir.join(name);
        if write {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            drift.push(name);
        }
    }
    assert!(
        drift.is_empty(),
        "docs/registries differs from crates/device/src/registry.rs: {drift:?}\n\
         regenerate: MC_WRITE_REGISTRIES=1 cargo test -p maker-connect-device --test registry_docs"
    );
}
