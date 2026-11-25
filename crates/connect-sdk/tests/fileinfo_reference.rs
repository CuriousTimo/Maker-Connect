//! Ours (`src/fileinfo.rs`) against the SDK's `fileinfo.py` run on the same steps:
//! `tests/fileinfo_scenario.py` runs the reference and prints what it gave; this test runs ours
//! the same way and wants the same.
//!
//! The Python, with the SDK (c0d7d233) and gcode-metadata (7d8f25d) installed, on CPython
//! 3.13.12: `FILEINFO_PYTHON`, else `reference/.venv-connect-sdk-313`; gcode-metadata's test
//! files: `FILEINFO_GCODES`, else `reference/gcode-metadata/tests/gcodes`. Without them it says
//! so and passes. On Windows the reference is CPython with posixpath as os.path
//! (`tests/pyos_windows.py`).

use maker_connect_sdk::fileinfo::{self, Error};
use maker_connect_sdk::pyos;
use maker_connect_sdk::util::py_json_dumps;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The start of a `-c` check: on Windows the scenarios' model of CPython there
/// (`tests/pyos_windows.py`), as the scenarios run it before they import the SDK.
fn windows_model() -> String {
    format!(
        "import os\nif os.name == 'nt':\n    exec(open(r'{}').read())\n",
        manifest("tests/pyos_windows.py").display()
    )
}

fn setup() -> Option<(String, PathBuf)> {
    let python = std::env::var("FILEINFO_PYTHON").ok().unwrap_or_else(|| {
        manifest("../../reference/.venv-connect-sdk-313/bin/python")
            .to_string_lossy()
            .into_owned()
    });
    let gcodes = std::env::var("FILEINFO_GCODES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest("../../reference/gcode-metadata/tests/gcodes"));
    let check = Command::new(&python)
        .args([
            "-I",
            "-c",
            &*format!(
                "{}{}",
                windows_model(),
                "import sys, prusa.connect.printer.fileinfo; print(sys.version.split()[0])"
            ),
        ])
        .output();
    match check {
        Ok(out) if out.status.success() && gcodes.is_dir() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(version, "3.13.12", "the reference's Python");
            Some((python, gcodes))
        }
        _ => {
            // CI's job `references` sets REFERENCES_REQUIRED: there a skip is a failure.
            assert!(
                std::env::var_os("REFERENCES_REQUIRED").is_none(),
                "REFERENCES_REQUIRED: no Python with the SDK (FILEINFO_PYTHON) or no gcode-metadata test files"
            );
            eprintln!(
                "skipped: no Python with the SDK (FILEINFO_PYTHON) or no gcode-metadata test files"
            );
            None
        }
    }
}

fn py(python: &str, args: &[&Path], mode: &str) -> Value {
    let out = Command::new(python)
        .arg("-I")
        .arg(manifest("tests/fileinfo_scenario.py"))
        .arg(mode)
        // in pyos's form (on Windows `/`), as ours sees the paths
        .args(args.iter().map(|p| String::from_utf8(b(p)).unwrap()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if out.stdout.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

fn b(p: &Path) -> Vec<u8> {
    pyos::path_bytes(p)
}

fn outcome<T>(r: Result<T, Error>, ok: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => json!(["ok", ok(v)]),
        Err(Error::Rejected(reason)) => json!(["CommandRejected", reason]),
        Err(err) => json!([err.repr()]),
    }
}

fn dumps(info: Map<String, Value>) -> Value {
    json!(py_json_dumps(&Value::Object(info)))
}

const NAMES: [&str; 18] = [
    "a.gcode",
    "a.GCODE",
    "a.Gc",
    "a.g",
    "a.gco",
    "a.gcode.txt",
    "fw.hex",
    "fw.HEX",
    "b.txt",
    "gcode",
    ".gcode",
    "a.bgcode",
    "a.sl1",
    "",
    "a.g\u{0130}",
    "a.\u{212a}gcode",
    "\u{00e9}.gcode",
    "a.gcodE",
];

fn listdir_sorted(p: &Path) -> Vec<Vec<u8>> {
    let mut names: Vec<Vec<u8>> = std::fs::read_dir(p)
        .unwrap()
        .map(|e| b(Path::new(&e.unwrap().file_name())))
        .collect();
    names.sort();
    names
}

fn ours(shared: &Path, copy: &Path) -> Value {
    let s = shared.join("storage");
    let sb = b(&s);
    let mut steps = Vec::new();
    let mut step = |name: String, v: Value| steps.push(json!([name, v]));

    step(
        "get_type_by_name".into(),
        NAMES
            .iter()
            .map(|n| {
                json!([
                    fileinfo::get_type_by_name(n.as_bytes(), false).value(),
                    fileinfo::get_type_by_name(n.as_bytes(), true).value()
                ])
            })
            .collect(),
    );
    let inside = [
        sb.clone(),
        [&sb[..], b"/"].concat(),
        b(&shared.join("storage_link")),
        b(&shared.join("outside")),
    ];
    let mut entries = listdir_sorted(&s);
    for extra in [
        &b"missing"[..],
        b"models/a.gcode",
        b"models/../..",
        b"link_in/sub",
        b"link_back/../../outside",
    ] {
        entries.push(extra.to_vec());
    }
    step(
        "is_inside".into(),
        entries
            .iter()
            .map(|p| {
                let path = pyos::join(&sb, &[p]);
                Value::Array(
                    inside
                        .iter()
                        .map(|st| {
                            outcome(fileinfo::is_inside(&path, st).map_err(Error::Os), |v| {
                                json!(v)
                            })
                        })
                        .collect(),
                )
            })
            .collect(),
    );

    let folder = |virtual_path: &str, storage: &[u8], os_path: &[u8]| {
        outcome(
            pyos::stat(os_path)
                .map_err(Error::Os)
                .and_then(|st| fileinfo::folder_info(virtual_path, storage, os_path, &st)),
            dumps,
        )
    };
    for d in ["", "models", "models/sub", "empty", "link_in", "link_back"] {
        step(
            format!("folder_info {d}"),
            folder(
                &format!("/local/{d}"),
                &sb,
                &pyos::join(&sb, &[d.as_bytes()]),
            ),
        );
    }
    step(
        "folder_info, storage by its link".into(),
        folder("/local", &b(&shared.join("storage_link")), &sb),
    );
    let over = b(&shared.join("over"));
    let limit = b(&shared.join("limit"));
    step("folder_info over".into(), folder("/over", &over, &over));
    step("folder_info limit".into(), folder("/limit", &limit, &limit));
    step(
        "folder_info a file".into(),
        folder("/local/models/a.gcode", &sb, &b(&s.join("models/a.gcode"))),
    );
    step(
        "folder_info missing".into(),
        outcome(
            pyos::stat(&sb).map_err(Error::Os).and_then(|st| {
                fileinfo::folder_info("/local/missing", &sb, &b(&s.join("missing")), &st)
            }),
            dumps,
        ),
    );

    let models = copy.join("models");
    let file = |name: &str| {
        let os_path = b(&models.join(name));
        outcome(
            pyos::stat(&os_path).map_err(Error::Os).and_then(|st| {
                fileinfo::file_info(&format!("/local/models/{name}"), &os_path, &st)
            }),
            dumps,
        )
    };
    let mut names: Vec<String> = listdir_sorted(&models)
        .into_iter()
        .map(|n| String::from_utf8(n).unwrap())
        .collect();
    names.push("sub/c.g".into());
    let files: Vec<&String> = names.iter().filter(|n| !models.join(n).is_dir()).collect();
    for n in &files {
        step(format!("file_info {n}"), file(n));
    }
    for n in &files {
        step(format!("file_info {n} again"), file(n));
    }
    step(
        "file_info removed".into(),
        outcome(
            pyos::stat(&b(&models.join("b.txt")))
                .map_err(Error::Os)
                .and_then(|st| {
                    fileinfo::file_info(
                        "/local/models/gone.gcode",
                        &b(&models.join("gone.gcode")),
                        &st,
                    )
                }),
            dumps,
        ),
    );
    let mut caches: Vec<String> = listdir_sorted(&models)
        .into_iter()
        .map(|n| String::from_utf8(n).unwrap())
        .filter(|n| n.starts_with('.'))
        .collect();
    caches.sort();
    step("caches".into(), json!(caches));
    json!(steps)
}

fn chmod(p: &Path, mode: u32) {
    if !p.exists() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    // CPython's os.chmod on Windows: only the read-only attribute, set without `S_IWRITE`
    #[cfg(windows)]
    {
        let mut perm = std::fs::metadata(p).unwrap().permissions();
        perm.set_readonly(mode & 0o200 == 0);
        std::fs::set_permissions(p, perm).unwrap();
    }
}

fn cleanup(base: &Path) {
    for copy in ["ref", "ours"] {
        chmod(&base.join(copy).join("models/locked.gcode"), 0o644);
    }
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn as_the_reference() {
    let Some((python, gcodes)) = setup() else {
        return;
    };
    let base = std::env::temp_dir().join(format!("fileinfo-test-{}", std::process::id()));
    cleanup(&base);
    std::fs::create_dir_all(&base).unwrap();
    let shared = base.join("shared");
    let (theirs, mine) = (base.join("ref"), base.join("ours"));
    py(&python, &[&shared, &gcodes], "build-shared");
    py(&python, &[&theirs, &gcodes], "build-copy");
    py(&python, &[&mine, &gcodes], "build-copy");

    let want = py(&python, &[&shared, &theirs], "run");
    let got = ours(&shared, &mine);
    let (w, g) = (want["steps"].as_array().unwrap(), got.as_array().unwrap());
    for (w, g) in w.iter().zip(g) {
        assert_eq!(g, w);
    }
    assert_eq!(g.len(), w.len(), "the number of steps");
    cleanup(&base);
}
