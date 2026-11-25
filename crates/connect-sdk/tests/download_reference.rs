//! Ours (`src/download.rs`) against the SDK's `download.py` run on the same steps:
//! `tests/download_scenario.py run` runs the reference and prints what it did; this test runs
//! ours the same way and wants the same. Both download from one server
//! (`download_scenario.py serve`, stdlib only), which logs the headers it got.
//!
//! The Python, with the SDK at c0d7d233 installed, on CPython 3.13.12: `DOWNLOAD_PYTHON`, else
//! `reference/.venv-connect-sdk-313`. Without it it says so and passes.
//!
//! One step's text is not compared: "unreachable", whose reason is `str()` of a `requests`
//! exception (urllib3's words); both must give one. One difference is known and checked to be
//! exactly that: "gzip", whose `size` ours does not have (see `gzip_size`).

#![cfg(unix)]

use maker_connect_sdk::command::Finish;
use maker_connect_sdk::consts::StorageType;
use maker_connect_sdk::consts::{Event, TransferType};
use maker_connect_sdk::download::{DownloadMgr, Transfer};
use maker_connect_sdk::files::{File, Filesystem};
use maker_connect_sdk::models::EventCallback;
use serde_json::{Map, Value, json};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn manifest(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn python() -> Option<String> {
    let python = std::env::var("DOWNLOAD_PYTHON").ok().unwrap_or_else(|| {
        manifest("../../reference/.venv-connect-sdk-313/bin/python")
            .to_string_lossy()
            .into_owned()
    });
    let check = Command::new(&python)
        .args([
            "-I",
            "-c",
            "import sys, prusa.connect.printer.download; print(sys.version.split()[0])",
        ])
        .output();
    match check {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(version, "3.13.12", "the reference's Python");
            Some(python)
        }
        _ => {
            // CI's job `references` sets REFERENCES_REQUIRED: there a skip is a failure.
            assert!(
                std::env::var_os("REFERENCES_REQUIRED").is_none(),
                "REFERENCES_REQUIRED: skipped: no Python with the SDK (DOWNLOAD_PYTHON)"
            );
            eprintln!("skipped: no Python with the SDK (DOWNLOAD_PYTHON)");
            None
        }
    }
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-download-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // the path the reference sees (`os.path.abspath`, no symlinks resolved)
    d
}

fn mask(v: &Value, d: &str) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, x)| {
                    let x = if k == "transfer_id" && x.is_number() {
                        json!("<id>")
                    } else if k == "time_transferring" {
                        json!("<t>")
                    } else {
                        mask(x, d)
                    };
                    (k.clone(), x)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|x| mask(x, d)).collect()),
        Value::String(s) => Value::String(s.replace(d, "<D>")),
        other => other.clone(),
    }
}

fn files(d: &Path) -> Value {
    fn walk(top: &Path, d: &Path, out: &mut Vec<Value>) {
        let mut entries: Vec<_> = std::fs::read_dir(top)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        let (dirs, names): (Vec<_>, Vec<_>) = entries.into_iter().partition(|p| p.is_dir());
        for p in names {
            let data = std::fs::read(&p).unwrap();
            use sha2::Digest;
            let hash: String = sha2::Sha256::digest(&data)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let rel = p.strip_prefix(d).unwrap().to_string_lossy().into_owned();
            out.push(json!([rel, data.len(), hash]));
        }
        for p in dirs {
            walk(&p, d, out);
        }
    }
    let mut out = Vec::new();
    walk(d, d, &mut out);
    Value::Array(out)
}

fn take_log(log: &Path) -> Value {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    std::fs::write(log, "").unwrap();
    Value::Array(
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

/// The dict `DownloadMgr.start` returns: `Transfer.start`'s `to_dict()` with `event` and
/// `source`, or the REJECTED dict.
fn started(finish: &Finish) -> Value {
    let mut out = Map::new();
    if finish.event == Some(Event::Rejected) {
        out.insert("event".into(), json!(Event::Rejected.value()));
        out.insert("source".into(), json!(finish.source.value()));
        out.extend(finish.kwargs.clone());
    } else {
        out.extend(finish.kwargs.clone());
        out.insert("event".into(), json!(finish.event.map(|e| e.value())));
        out.insert("source".into(), json!(finish.source.value()));
    }
    Value::Object(out)
}

#[derive(Default)]
struct Step<'a> {
    server: Option<Option<String>>,
    type_: Option<TransferType>,
    printed: Option<String>,
    stop_at_first_chunk: bool,
    transfer: Option<Arc<Transfer>>,
    no_loop: bool,
    again: bool,
    url: &'a str,
}

struct Ours {
    base: String,
    log: PathBuf,
    d: PathBuf,
    usb: String,
    records: Vec<Value>,
}

impl Ours {
    fn step(&mut self, name: &str, path: &str, s: Step) -> Arc<Transfer> {
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        let transfer = s.transfer.unwrap_or_else(|| Arc::new(Transfer::new()));
        // The scenario's stand-in `Fs` (its `sep` and each storage's `path_storage`): ours is
        // the ported `Filesystem` with the same storages, attached without watching.
        let fs = Arc::new(Filesystem::new("/", None));
        for (name, path_storage) in [
            ("usb", self.usb.clone()),
            ("nopath", String::new()),
            ("slash", format!("{}/", self.usb)),
        ] {
            let tree = File::node(name, true);
            fs.attach(name, tree, &path_storage, false, StorageType::Local, None)
                .unwrap();
        }
        let server = s.server.unwrap_or_else(|| Some(self.base.clone()));
        let headers: Vec<(String, Option<String>)> = [
            ("Token", "secret"),
            ("User-Agent", "UA/1.0"),
            ("Fingerprint", "fp"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), Some((*v).to_owned())))
        .collect();
        let log_events = events.clone();
        let event_cb: EventCallback = Arc::new(move |event, source, _ts, _id, kwargs| {
            log_events.lock().unwrap().push(json!([
                event.value(),
                source.value(),
                Value::Object(kwargs)
            ]));
        });
        let finished_events = events.clone();
        let printed = s.printed.clone();
        let mgr = Arc::new(DownloadMgr::new(
            fs,
            transfer.clone(),
            Arc::new(move || (server.clone(), headers.clone())),
            event_cb,
            Arc::new(move || printed.clone()),
            Arc::new(move |t: &Transfer| {
                finished_events
                    .lock()
                    .unwrap()
                    .push(json!(["finished_cb", t.data().path.clone()]));
            }),
        ));
        if s.stop_at_first_chunk {
            let weak = Arc::downgrade(&transfer);
            transfer.set_progress_cb(Arc::new(move || {
                if let Some(t) = weak.upgrade()
                    && t.transferred() != 0
                {
                    t.stop();
                }
            }));
        }
        let type_ = s.type_.unwrap_or(TransferType::FromConnect);
        let url = format!("{}{}", self.base, s.url);
        let url = if s.url.starts_with("http") {
            s.url.to_owned()
        } else {
            url
        };
        let start = match mgr.start(
            type_,
            path,
            Some(&url),
            Some(true),
            Some(false),
            Some(7),
            Some("abc"),
            Some(json!(3)),
        ) {
            Ok(finish) => started(&finish),
            Err(err) => json!([err.repr()]),
        };
        let again = if s.again {
            match mgr.start(type_, path, Some(&url), None, None, Some(8), None, None) {
                Ok(finish) => started(&finish),
                Err(err) => json!([err.repr()]),
            }
        } else {
            Value::Null
        };
        if !s.no_loop && transfer.in_progress() {
            let looping = mgr.clone();
            let thread = std::thread::spawn(move || looping.run_loop());
            let deadline = Instant::now() + Duration::from_secs(30);
            while transfer.in_progress() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            mgr.stop_loop();
            thread.join().unwrap();
        }
        let events = Value::Array(events.lock().unwrap().clone());
        let record = json!({
            "step": name,
            "start": start,
            "again": again,
            "events": events,
            "files": files(&self.d),
            "requests": take_log(&self.log),
        });
        self.records.push(mask(&record, &self.d.to_string_lossy()));
        transfer
    }
}

fn run_ours(port: u16, log: &Path, d: &Path) -> Vec<Value> {
    let base = format!("http://127.0.0.1:{port}");
    let usb = d.join("usb");
    std::fs::create_dir_all(&usb).unwrap();
    std::fs::write(usb.join("file"), b"a file where a folder is wanted").unwrap();
    let mut o = Ours {
        base: base.clone(),
        log: log.to_owned(),
        d: d.to_owned(),
        usb: usb.to_string_lossy().into_owned(),
        records: Vec::new(),
    };
    let no_loop = |url| Step {
        url,
        no_loop: true,
        ..Step::default()
    };
    let e125 = format!("/usb/{}.gcode", "é".repeat(125));
    let a249 = format!("/usb/{}", "a".repeat(249));
    let f256 = format!("/usb/{}/a.gcode", "f".repeat(256));
    let e255 = format!("/usb/{}/a.gcode", "é".repeat(255));
    for p in [
        "usb/a.gcode",
        "/usb",
        "/nope/a.gcode",
        "/nopath/a.gcode",
        "/usb/../x.gcode",
        "/usb/a#.gcode",
        "/usb/a?.gcode",
        e125.as_str(),
        a249.as_str(),
        f256.as_str(),
        e255.as_str(),
        "/slash/s.gcode",
        "/usb//b.gcode",
        "/usb/./c.gcode",
    ] {
        // Python's `"start " + p[:40]`
        let name = format!("start {}", p.chars().take(40).collect::<String>());
        o.step(&name, p, no_loop("/ok.gcode"));
    }
    let url = |url| Step {
        url,
        ..Step::default()
    };
    o.step("connect, token kept", "/usb/a.gcode", url("/ok.gcode"));
    o.step(
        "another server: User-Agent only",
        "/usb/b.gcode",
        Step {
            server: Some(Some("http://example.invalid".into())),
            ..url("/ok.gcode")
        },
    );
    o.step(
        "no server",
        "/usb/b2.gcode",
        Step {
            server: Some(None),
            ..url("/ok.gcode")
        },
    );
    o.step(
        "server in upper case",
        "/usb/b3.gcode",
        Step {
            server: Some(Some(base.to_uppercase())),
            ..url("/ok.gcode")
        },
    );
    o.step("text/plain", "/usb/c.gcode", url("/plain.gcode"));
    o.step("404", "/usb/d.gcode", url("/missing"));
    o.step("text/html", "/usb/e.gcode", url("/html"));
    o.step("a charset", "/usb/e2.gcode", url("/charset"));
    o.step("upper-case type", "/usb/e3.gcode", url("/upper"));
    o.step("no content type", "/usb/f.gcode", url("/no-type"));
    o.step("empty", "/usb/g.gcode", url("/empty"));
    o.step("chunked, no length", "/usb/h.gcode", url("/chunked"));
    o.step("gzip", "/usb/i.gcode", url("/gzip"));
    o.step("deflate", "/usb/j.gcode", url("/deflate"));
    o.step(
        "into new folders",
        "/usb/sub/deeper/k.gcode",
        url("/ok.gcode"),
    );
    o.step(
        "into an existing folder",
        "/usb/sub/deeper/l.gcode",
        url("/ok.gcode"),
    );
    o.step(
        "a file where the folder is",
        "/usb/file/m.gcode",
        url("/ok.gcode"),
    );
    o.step(
        "stopped",
        "/usb/n.gcode",
        Step {
            stop_at_first_chunk: true,
            ..url("/big")
        },
    );
    o.step(
        "the printed file",
        "/usb/a.gcode",
        Step {
            printed: Some(usb.join("a.gcode").to_string_lossy().into_owned()),
            ..url("/ok.gcode")
        },
    );
    o.step(
        "from the web",
        "/usb/o.gcode",
        Step {
            type_: Some(TransferType::FromWeb),
            ..url("/ok.gcode")
        },
    );
    o.step(
        "not a download type",
        "/usb/p.gcode",
        Step {
            type_: Some(TransferType::FromClient),
            no_loop: true,
            ..url("/ok.gcode")
        },
    );
    o.step(
        "another transfer in progress",
        "/usb/q.gcode",
        Step {
            again: true,
            ..url("/ok.gcode")
        },
    );
    o.step("unreachable", "/usb/r.gcode", url("http://127.0.0.1:9/x"));
    o.step("invalid storage, loop", "/nope/s.gcode", url("/ok.gcode"));
    let t = o.step("then a good one", "/usb/t.gcode", url("/ok.gcode"));
    o.step(
        "then an invalid storage again",
        "/nope/u.gcode",
        Step {
            transfer: Some(t),
            ..url("/ok.gcode")
        },
    );
    o.records
}

/// The one known difference (see the module doc): a gzip response's `Content-Length`, which
/// ureq drops when it decompresses (`ureq-2.12.1` `src/response.rs:606-609`), so ours has no
/// `size` where the reference has the compressed length. Checked to be exactly that.
fn gzip_size(got: &mut [Value], want: &[Value]) {
    for (g, w) in got
        .iter_mut()
        .zip(want)
        .filter(|(_, w)| w["step"] == "gzip")
    {
        let size = &mut g["events"][0][2]["size"];
        assert!(size.is_null(), "ours: no size for gzip");
        assert!(
            w["events"][0][2]["size"].is_u64(),
            "ref: the compressed length"
        );
        *size = w["events"][0][2]["size"].clone();
    }
}

/// The one reason not compared (see the module doc): both must have one.
fn unreachable_reason(records: &mut [Value]) {
    for r in records.iter_mut().filter(|r| r["step"] == "unreachable") {
        for e in r["events"].as_array_mut().unwrap() {
            if let Some(reason) = e[2].get_mut("reason") {
                assert!(reason.is_string(), "a reason");
                *reason = json!("<requests' words>");
            }
        }
    }
}

#[test]
fn as_the_reference() {
    let Some(python) = python() else {
        return;
    };
    let work = tmpdir("work");
    let log = work.join("requests.log");
    std::fs::write(&log, "").unwrap();
    let mut child = Command::new(&python)
        .arg("-I")
        .arg(manifest("tests/download_scenario.py"))
        .arg("serve")
        .arg(&log)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let _server = Server(child);
    let port: u16 = line.trim().parse().unwrap();

    let ref_d = tmpdir("ref");
    let out = Command::new(&python)
        .arg("-I")
        .arg(manifest("tests/download_scenario.py"))
        .arg("run")
        .arg(port.to_string())
        .arg(&log)
        .arg(&ref_d)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut want: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    let ours_d = tmpdir("ours");
    let mut got = run_ours(port, &log, &ours_d);
    unreachable_reason(&mut want);
    unreachable_reason(&mut got);
    gzip_size(&mut got, &want);

    assert_eq!(got.len(), want.len(), "the number of steps");
    let mut differ = Vec::new();
    for (g, w) in got.iter().zip(&want) {
        // compared as text, so the order of the keys counts too
        let (gs, ws) = (g.to_string(), w.to_string());
        if gs != ws {
            differ.push(format!("{}\n  ours: {gs}\n  ref:  {ws}", w["step"]));
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
    eprintln!("{} steps the same", want.len());
    for d in [work, ref_d, ours_d] {
        let _ = std::fs::remove_dir_all(d);
    }
}
