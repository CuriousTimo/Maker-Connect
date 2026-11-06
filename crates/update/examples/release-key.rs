//! The release key: made once, kept by who releases, never in the repository.
//!
//!   cargo run -p maker-connect-update --example release-key -- new <key file>
//!   cargo run -p maker-connect-update --example release-key -- public <key file>
//!   cargo run -p maker-connect-update --example release-key -- sign <key file> <tag> <SHA256SUMS>
//!
//! `new` writes a new Ed25519 key (PKCS#8, readable by its owner only) and prints its public
//! half, for `RELEASE_KEY` in crates/update/src/lib.rs. `sign` writes `<file>.sig` beside the
//! list (base64, over `<tag>\n<list>`), which `scripts/build-release.sh` does for every release.

use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};

fn pair(path: &str) -> Ed25519KeyPair {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    Ed25519KeyPair::from_pkcs8(&bytes).expect("not an Ed25519 key in PKCS#8")
}

fn print_public(pair: &Ed25519KeyPair) {
    let bytes: Vec<String> = pair
        .public_key()
        .as_ref()
        .iter()
        .map(|b| format!("0x{b:02x}"))
        .collect();
    println!("pub const RELEASE_KEY: [u8; 32] = [{}];", bytes.join(", "));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["new", path] => {
            if std::path::Path::new(path).exists() {
                panic!("{path} exists: a key is made once");
            }
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("no randomness");
            if let Some(dir) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(path, pkcs8.as_ref()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            eprintln!("made {path}: keep a copy somewhere safe, and never commit it");
            print_public(&pair(path));
        }
        ["public", path] => print_public(&pair(path)),
        ["sign", path, tag, file] => {
            let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{file}: {e}"));
            let signature =
                pair(path).sign(maker_connect_update::signed_text(tag, &text).as_bytes());
            let out = format!("{file}.sig");
            std::fs::write(
                &out,
                base64::engine::general_purpose::STANDARD.encode(signature) + "\n",
            )
            .unwrap();
            eprintln!("signed: {out}");
        }
        _ => {
            eprintln!("release-key new <key> | public <key> | sign <key> <tag> <SHA256SUMS>");
            std::process::exit(2);
        }
    }
}
