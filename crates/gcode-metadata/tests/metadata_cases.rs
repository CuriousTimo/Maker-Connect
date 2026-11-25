//! Our gcode-metadata against the reference (prusa3d/gcode-metadata at 7d8f25d on CPython 3.13.12)
//! on the reference's own test files (`reference/gcode-metadata/tests/gcodes/`): the whole file
//! with `get_metadata(path, False)`, and the file fed to `load_from_chunk` in chunks
//! (`tests/gcode-metadata-cases.json`, recorded by `scripts/gcode-metadata-cases.py`).
//!
//! The files are the reference's and live in `reference/` (not in the repository, nor in CI):
//! without them the test says so and passes; `scripts/check.sh` runs it where they are.

use maker_connect_gcode_metadata::metadata::{MetaData, MetaError, get_meta_class, get_metadata};
use maker_connect_gcode_metadata::py::{self, PyValue};
use serde_json::{Value, json};
use sha2::Digest;
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom};

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn shown(meta: &MetaData) -> Value {
    let thumbnails: Vec<Value> = meta
        .thumbnails
        .keys()
        .zip(meta.thumbnails.values())
        .map(|(k, v)| json!([k, v.len(), hex(&sha2::Sha256::digest(v))]))
        .collect();
    json!({
        "data": py::json_dumps(&PyValue::Dict(meta.data.clone()), None),
        "thumbnails": thumbnails,
    })
}

fn outcome(result: Result<Value, MetaError>) -> Value {
    match result {
        Ok(v) => json!(["ok", v]),
        Err(MetaError::UnknownGcodeFileType(path)) => json!(["UnknownGcodeFileType", path]),
        Err(MetaError::Py(e)) => json!([e.class_name(), e.message()]),
        Err(MetaError::Os(_)) => json!(["OSError", null]),
    }
}

/// `chunk_read_file` of the reference's tests.
fn chunked(path: &str, size: usize) -> Value {
    outcome((|| {
        let mut meta = get_meta_class(path, None)?;
        let mut file = std::fs::File::open(path)?;
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        let mut chunk = vec![0u8; size];
        loop {
            let mut got = 0;
            while got < size {
                let n = file.read(&mut chunk[got..])?;
                if n == 0 {
                    break;
                }
                got += n;
            }
            if got == 0 {
                break;
            }
            meta.load_from_chunk(&chunk[..got], file_size)?;
        }
        Ok(shown(&meta))
    })())
}

#[test]
fn metadata_as_the_reference() {
    let dir = env!("CARGO_MANIFEST_DIR");
    // CI: the checkout's folder in GCODE_METADATA_GCODES, else the local reference/
    let gcodes = std::env::var("GCODE_METADATA_GCODES")
        .unwrap_or_else(|_| format!("{dir}/../../reference/gcode-metadata/tests/gcodes"));
    if !std::path::Path::new(&gcodes).is_dir() {
        // CI's job `references` sets REFERENCES_REQUIRED: there a skip is a failure.
        assert!(
            std::env::var_os("REFERENCES_REQUIRED").is_none(),
            "REFERENCES_REQUIRED: skipped: the reference's test files are not here ({gcodes})"
        );
        eprintln!("skipped: the reference's test files are not here ({gcodes})");
        return;
    }
    let text = std::fs::read_to_string(format!("{dir}/tests/gcode-metadata-cases.json")).unwrap();
    let recorded: Value = serde_json::from_str(&text).unwrap();
    let mut differ = Vec::new();
    let cases = recorded["cases"].as_object().unwrap();
    assert!(cases.len() >= 9);
    for (name, want) in cases {
        let path = format!("{gcodes}/{name}");
        let mut want = want.clone();
        // An `OSError`'s text is the platform's: only its class is compared.
        let by_class = |v: &mut Value| {
            if v[0] == "OSError" {
                v[1] = Value::Null;
            }
        };
        by_class(&mut want["whole"]);
        want["chunks"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .for_each(by_class);
        let whole = outcome(get_metadata(&path, false, None).map(|m| shown(&m)));
        if whole != want["whole"] {
            differ.push(format!(
                "{name} whole:\n  reference {}\n  ours      {whole}",
                want["whole"]
            ));
        }
        for (size, want) in want["chunks"].as_object().unwrap() {
            let got = chunked(&path, size.parse().unwrap());
            if &got != want {
                differ.push(format!(
                    "{name} chunks of {size}:\n  reference {want}\n  ours      {got}"
                ));
            }
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}
