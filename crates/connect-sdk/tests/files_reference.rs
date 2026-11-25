//! Ours (`src/files.rs`) against the SDK's `files.py` run on the same steps:
//! `tests/files_scenario.py` runs the reference and prints what it gave; this test runs ours
//! the same way and wants the same.
//!
//! - `fs`: a `Filesystem` of a built folder without watching: the tree, `get`, `get_os_path`,
//!   `get_abs_os_path`, `file_info`, `attach`/`detach`, `Storage`, `common_start`,
//!   `get_relative_paths`, `update`, `delete`. On Linux and macOS.
//! - `inotify`: the same folder watched, one file action after another (the script's `STEPS`,
//!   done by the script on both folders), and after each what the handler made of it: its
//!   result, the events to Connect, the tree, the number of watches. On Linux only: the
//!   reference reads inotify (`inotify_simple`), ours notify's events (`src/files.rs`), which on
//!   macOS come from FSEvents.
//!
//! The Python, with the SDK (c0d7d233) and gcode-metadata (7d8f25d) installed, on CPython
//! 3.13.12: `FILES_PYTHON`, else `reference/.venv-connect-sdk-313`. Without it it says so and
//! passes.

#![cfg(unix)]

use maker_connect_sdk::consts::StorageType;
use maker_connect_sdk::files::{self, File, Filesystem, InotifyHandler, Storage};
#[cfg(target_os = "linux")]
use maker_connect_sdk::models::EventCallback;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::time::Duration;

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn setup() -> Option<String> {
    let python = std::env::var("FILES_PYTHON").ok().unwrap_or_else(|| {
        manifest("../../reference/.venv-connect-sdk-313/bin/python")
            .to_string_lossy()
            .into_owned()
    });
    let check = Command::new(&python)
        .args([
            "-I",
            "-c",
            "import sys, prusa.connect.printer.files; print(sys.version.split()[0])",
        ])
        .output();
    match check {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(version, "3.13.12", "the reference's Python");
            Some(python)
        }
        _ => {
            eprintln!("skipped: no Python with the SDK (FILES_PYTHON)");
            None
        }
    }
}

fn py(python: &str, args: &[&str]) -> Value {
    let out = Command::new(python)
        .arg("-I")
        .arg(manifest("tests/files_scenario.py"))
        .args(args)
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

/// The script's `norm`: the folder as `<D>`, a time `T`, a size of the disk `N`.
fn norm(value: Value, d: &str) -> Value {
    let d = d.trim_end_matches('/');
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let v = match k.as_str() {
                        "m_timestamp" => json!("T"),
                        "free_space" | "total_space" => json!("N"),
                        _ => norm(v, d),
                    };
                    (k, v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| norm(v, d)).collect()),
        Value::String(s) => Value::String(s.replace(d, "<D>")),
        other => other,
    }
}

/// The script's `call`: what a call gave, or `["raised", repr(err)]`.
fn call<T: Into<Value>, E>(d: &str, r: Result<T, E>, repr: impl Fn(&E) -> String) -> Value {
    match r {
        Ok(v) => norm(v.into(), d),
        Err(err) => json!(["raised", norm(json!(repr(&err)), d)]),
    }
}

fn unit(r: Result<(), files::Error>) -> Result<Value, files::Error> {
    r.map(|()| Value::Null)
}

fn maps(r: Result<Vec<Map<String, Value>>, files::Error>) -> Result<Value, files::Error> {
    r.map(|v| Value::Array(v.into_iter().map(Value::Object).collect()))
}

/// The script's `dump`: every node of every storage, `[virtual path, to_dict(False)]`.
fn dump(fs: &Filesystem) -> Value {
    fn walk(node: &File, path: String, out: &mut Vec<Value>) {
        out.push(json!([path, node.to_dict(false)]));
        let mut children = node.children();
        children.sort_by_key(File::name);
        for child in children {
            walk(&child, format!("{path}/{}", child.name()), out);
        }
    }
    let mut out = Vec::new();
    let storages: Vec<(String, File)> = fs
        .storage_dict()
        .iter()
        .map(|(name, s)| (name.clone(), s.tree.clone()))
        .collect();
    for (name, tree) in storages {
        walk(&tree, format!("/{name}"), &mut out);
    }
    Value::Array(out)
}

fn add(parent: &File, name: &str, is_dir: bool, attrs: Value) -> File {
    let node = parent.add(name, is_dir).unwrap();
    let Value::Object(attrs) = attrs else {
        unreachable!()
    };
    node.set_attrs(attrs);
    node
}

fn fs_ours(d: &str) -> Value {
    let e = |err: &files::Error| err.repr();
    let fs = Arc::new(Filesystem::new("/", None));
    let mut out = Map::new();
    out.insert(
        "from_dir".into(),
        call(d, unit(fs.from_dir(d, "test", StorageType::Local, None)), e),
    );
    out.insert("tree".into(), norm(dump(&fs), d));
    out.insert("storages".into(), call(d, maps(fs.storages()), e));
    let storage_str = fs.storage_dict()["test"].str();
    out.insert(
        "storage_str".into(),
        json!(storage_str.replace(d.trim_end_matches('/'), "<D>")),
    );
    let gets = [
        "/test",
        "test",
        "/test/",
        "/test/a",
        "/test/a/1.gcode",
        "/test/a/c/3.txt",
        "/test/.h",
        "/test/a/.hid",
        "/test/a/.hid/x.gcode",
        "/test//a",
        "/nothing/a",
        "",
        "/",
        "/test/a/missing",
        "/test/\u{e9}.gcode",
    ];
    out.insert(
        "get".into(),
        gets.iter()
            .map(|p| json!([p, fs.get(p).map(|n| n.name())]))
            .collect(),
    );
    out.insert(
        "get_os_path".into(),
        gets.iter()
            .map(|p| json!([p, call(d, fs.get_os_path(p), e)]))
            .collect(),
    );
    // The script calls it on a stand-in with `fs` only; ours is a handler of the same `fs`.
    let handler = InotifyHandler::new(fs.clone()).unwrap();
    let abs_os = [
        "/test/a/1.gcode",
        "/test",
        "/test/",
        "test/a",
        "/nothing/a",
        "//test/a",
        "/test/../a",
        "/test/./a/",
        "/",
        ".",
    ];
    out.insert(
        "get_abs_os_path".into(),
        abs_os
            .iter()
            .map(|p| json!([p, call(d, handler.get_abs_os_path(p), e)]))
            .collect(),
    );
    drop(handler);
    let fe = |err: &maker_connect_sdk::fileinfo::Error| err.repr();
    let infos = [
        "/test",
        "/test/a",
        "/test/a/1.gcode",
        "/test/top.GCODE",
        "/test/fw.hex",
        "/test/a/missing",
        "/test/../x",
        "/test/.h",
        "/nothing",
        "/",
        "/test/a/1.gcode/x",
        "/test/empty",
    ];
    out.insert(
        "file_info".into(),
        infos
            .iter()
            .map(|p| json!([p, call(d, fs.file_info(p), fe)]))
            .collect(),
    );

    let tree = File::node("sd", true);
    add(
        &tree,
        "x.gcode",
        false,
        json!({"size": 5, "m_timestamp": 7}),
    );
    let sub = add(&tree, "sub", true, json!({}));
    add(&sub, "y.txt", false, json!({"size": 3, "read_only": true}));
    out.insert(
        "attach_sd".into(),
        call(
            d,
            unit(fs.attach("SD Card", tree, "", false, StorageType::Sdcard, Some(123))),
            e,
        ),
    );
    out.insert(
        "sd_info".into(),
        [
            "/SD Card",
            "/SD Card/sub",
            "/SD Card/x.gcode",
            "/SD Card/none",
            "/SD Card/sub/y.txt",
        ]
        .iter()
        .map(|p| json!([p, call(d, fs.file_info(p), fe)]))
        .collect(),
    );
    let sd = fs.storage_dict()["SD Card"].clone();
    out.insert("sd_storage".into(), call(d, sd.to_dict(), e));
    // `attach`'s defaults: `storage_path=""`, `use_inotify=True`, LOCAL, no system space.
    let empty = || File::node("e", true);
    let local = StorageType::Local;
    out.insert(
        "attach_errors".into(),
        json!([
            call(d, unit(fs.attach("", empty(), "", true, local, None)), e),
            call(d, unit(fs.attach("a/b", empty(), "", true, local, None)), e),
            call(
                d,
                unit(fs.attach("test", empty(), "", true, local, None)),
                e
            ),
            call(d, unit(fs.attach("x", empty(), "", true, local, None)), e),
            call(d, unit(fs.attach("y", empty(), "", true, local, None)), e),
        ]),
    );
    out.insert("storages_after".into(), call(d, maps(fs.storages()), e));
    out.insert(
        "detach".into(),
        json!([
            call(d, unit(fs.detach("x")), e),
            call(d, unit(fs.detach("x")), e),
            call(d, unit(fs.detach("SD Card")), e),
        ]),
    );
    out.insert(
        "storage_dict".into(),
        json!(fs.storage_dict().keys().cloned().collect::<Vec<_>>()),
    );
    let none = Storage::new(File::node("n", true), "n", None, false, local, None).unwrap();
    out.insert("storage_none".into(), json!(none.str()));

    out.insert(
        "common_start".into(),
        json!(
            [
                ("/a/b/c", "/a/b/d"),
                ("abc", "xyz"),
                ("", "x"),
                ("\u{e9}1", "\u{e9}2")
            ]
            .iter()
            .map(|(a, b)| files::common_start(a, b))
            .collect::<Vec<_>>()
        ),
    );
    let s = |v: &[&str]| v.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    out.insert(
        "relative".into(),
        json!([
            InotifyHandler::get_relative_paths("/tmp/r", &s(&["/tmp/r/folder"])).unwrap(),
            InotifyHandler::get_relative_paths(
                "/tmp/r",
                &s(&["/tmp/r", "/tmp/r/a", "/tmp/r/a/b", "/x/tmp/r/c", "/other"])
            )
            .unwrap(),
            InotifyHandler::get_relative_paths("/tmp/r/", &s(&["/tmp/r/", "/tmp/r/a"])).unwrap(),
        ]),
    );
    let node = File::node("n", true);
    Filesystem::update(
        &s(&["/s", "/s/a", "/s/a/b", "/s/.h", "/t/x"]),
        "/s",
        Some(&node),
    )
    .unwrap();
    out.insert(
        "update".into(),
        node.children()
            .iter()
            .map(|c| json!([c.name(), c.is_dir()]))
            .collect(),
    );
    let missing = format!("{}/missing", d.trim_end_matches('/'));
    out.insert(
        "delete_missing".into(),
        call(d, unit(files::delete(&missing, false, false)), e),
    );
    let a = format!("{}/a", d.trim_end_matches('/'));
    out.insert(
        "delete_dir_full".into(),
        call(d, unit(files::delete(&a, true, false)), e),
    );
    Value::Object(out)
}

/// The script's `STEPS`, in its order.
#[cfg(target_os = "linux")]
const STEPS: [&str; 16] = [
    "create_file",
    "mkdir",
    "file_in_folder",
    "append",
    "move_file",
    "rename",
    "move_dir",
    "file_in_moved_dir",
    "delete_file",
    "rmdir",
    "hidden",
    "many",
    "rmtree_many",
    "nested",
    "rmtree_c2",
    "delete_root",
];

/// The script's `WAIT`.
#[cfg(target_os = "linux")]
const WAIT: Duration = Duration::from_millis(300);

#[cfg(target_os = "linux")]
fn inotify_ours(python: &str, d: &str) -> Value {
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = events.clone();
    let event_cb: EventCallback = Arc::new(move |event, source, _, _, kwargs| {
        sink.lock()
            .unwrap()
            .push(json!([event.value(), source.value(), kwargs]));
    });
    let fs = Arc::new(Filesystem::new("/", Some(event_cb)));
    fs.from_dir(d, "test", StorageType::Local, None).unwrap();
    let handler = InotifyHandler::new(fs.clone()).unwrap();
    let taken = || Value::Array(std::mem::take(&mut *events.lock().unwrap()));
    let mut out = vec![json!({
        "step": "init", "events": norm(taken(), d), "tree": norm(dump(&fs), d),
        "wds": handler.wds().len(),
    })];
    for step in STEPS {
        taken();
        py(python, &["act", d, step]);
        std::thread::sleep(WAIT);
        let result = call(d, unit(handler.call(0)), |err| err.repr());
        out.push(json!({
            "step": step, "result": result, "events": norm(taken(), d),
            "tree": norm(dump(&fs), d), "wds": handler.wds().len(),
        }));
    }
    Value::Array(out)
}

/// Two copies of the same folder, the reference's and ours, under one base that is removed
/// afterwards.
struct Folders {
    base: PathBuf,
    theirs: String,
    mine: String,
}

impl Folders {
    fn new(python: &str, what: &str) -> Folders {
        let base = std::env::temp_dir().join(format!("files-test-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // The same path length on both sides: `<D>` stands in for either.
        let (theirs, mine) = (base.join("ref"), base.join("our"));
        let (theirs, mine) = (
            theirs.to_string_lossy().into_owned(),
            mine.to_string_lossy().into_owned(),
        );
        for d in [&theirs, &mine] {
            std::fs::create_dir(d).unwrap();
            py(python, &["build", d]);
        }
        Folders { base, theirs, mine }
    }
}

impl Drop for Folders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Every key (or step) where ours differs from the reference, all of them in one failure.
fn compare(want: &Value, got: &Value) {
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap();
    let mut differ = Vec::new();
    match (want, got) {
        (Value::Object(w), Value::Object(g)) => {
            for (k, w) in w {
                if g.get(k) != Some(w) {
                    differ.push(format!(
                        "key {k}\nreference: {}\nours: {}",
                        pretty(w),
                        g.get(k).map_or("missing".into(), pretty)
                    ));
                }
            }
            for k in g.keys().filter(|k| !w.contains_key(*k)) {
                differ.push(format!("key {k}: ours only"));
            }
        }
        (Value::Array(w), Value::Array(g)) => {
            for (i, (w, g)) in w.iter().zip(g).enumerate() {
                if g != w {
                    differ.push(format!(
                        "step {i}\nreference: {}\nours: {}",
                        pretty(w),
                        pretty(g)
                    ));
                }
            }
            if g.len() != w.len() {
                differ.push(format!("steps: reference {}, ours {}", w.len(), g.len()));
            }
        }
        _ if got != want => differ.push(format!(
            "reference: {}\nours: {}",
            pretty(want),
            pretty(got)
        )),
        _ => {}
    }
    assert!(
        differ.is_empty(),
        "{} difference(s):\n\n{}",
        differ.len(),
        differ.join("\n\n")
    );
}

#[test]
fn filesystem_as_the_reference() {
    let Some(python) = setup() else {
        return;
    };
    let folders = Folders::new(&python, "fs");
    let want = py(&python, &["fs", &folders.theirs]);
    let got = fs_ours(&folders.mine);
    compare(&want, &got);
}

#[cfg(target_os = "linux")]
#[test]
fn watching_as_the_reference() {
    let Some(python) = setup() else {
        return;
    };
    let folders = Folders::new(&python, "inotify");
    let want = py(&python, &["inotify", &folders.theirs]);
    let got = inotify_ours(&python, &folders.mine);
    compare(&want, &got);
}
