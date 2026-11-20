//! Ours against the reference (inotify_simple 1.3.5) run on the same steps: `tests/scenario.py`
//! runs the reference and prints what it gave; these tests run ours the same way and want the
//! same.
//!
//! The Python with the reference installed: `INOTIFY_SIMPLE_PYTHON`, else `PRUSA_SDK_PYTHON` (CI's
//! job `store`, which installs Prusa's SDK and so `inotify_simple~=1.3.5`), else
//! `reference/.venv-connect-sdk-313`. Without one the tests say so and pass. The session on a
//! real inotify fd runs on Linux only.

#![cfg(unix)]

use maker_connect_inotify_simple::{Error, Event, Flag, masks, parse_events};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt;

fn python() -> Option<String> {
    for var in ["INOTIFY_SIMPLE_PYTHON", "PRUSA_SDK_PYTHON"] {
        if let Ok(p) = std::env::var(var) {
            return Some(p);
        }
    }
    let venv = format!(
        "{}/../../reference/.venv-connect-sdk-313/bin/python",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::path::Path::new(&venv).exists() {
        return Some(venv);
    }
    eprintln!("skipped: no Python with inotify_simple (INOTIFY_SIMPLE_PYTHON, PRUSA_SDK_PYTHON)");
    None
}

/// `python -I scenario.py ARGS`, parsed.
fn reference(python: &str, args: &[&str]) -> Value {
    let script = format!("{}/tests/scenario.py", env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new(python)
        .arg("-I")
        .arg(script)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(got["version"], "1.3.5", "the reference's version");
    got
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn names(mask: u32) -> Vec<&'static str> {
    Flag::from_mask(mask).into_iter().map(Flag::name).collect()
}

/// `shown(events, cookies)` of the script.
fn shown(events: &[Event], cookies: Option<&mut Vec<u32>>) -> Value {
    let mut cookies = cookies;
    let out: Vec<Value> = events
        .iter()
        .map(|e| {
            let mut cookie = e.cookie;
            if let Some(seen) = cookies.as_deref_mut()
                && cookie != 0
            {
                cookie = match seen.iter().position(|c| *c == cookie) {
                    Some(i) => i as u32 + 1,
                    None => {
                        seen.push(cookie);
                        seen.len() as u32
                    }
                };
            }
            json!([e.wd, names(e.mask), cookie, hex(e.name.as_bytes())])
        })
        .collect();
    Value::Array(out)
}

fn outcome<T>(result: Result<T, Error>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(v) => json!(["ok", ok(v)]),
        Err(Error::Os(err)) => json!(["OSError", err.raw_os_error()]),
        Err(Error::Py { class, message }) => json!([class, message]),
    }
}

#[test]
fn pure_as_the_reference() {
    let Some(python) = python() else {
        return;
    };
    let want = reference(&python, &["pure"]);
    let parse: Vec<Value> = want["parse_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let data = unhex(case[0].as_str().unwrap());
            json!([case[0], outcome(parse_events(&data), |e| shown(&e, None))])
        })
        .collect();
    let from_mask: Vec<Value> = want["from_mask"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let mask = case[0].as_u64().unwrap() as u32;
            json!([mask, names(mask)])
        })
        .collect();
    let flags: Vec<Value> = Flag::MEMBERS
        .iter()
        .map(|f| json!([f.name(), f.bits()]))
        .collect();
    let ours = json!({
        "version": maker_connect_inotify_simple::VERSION,
        "parse_events": parse,
        "from_mask": from_mask,
        "flags": flags,
        "masks": [["CLOSE", masks::CLOSE], ["MOVE", masks::MOVE], ["ALL_EVENTS", masks::ALL_EVENTS]],
    });
    assert_eq!(ours, want);
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use maker_connect_inotify_simple::INotify;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    /// Prusa-Connect-SDK-Printer files.py: InotifyHandler.WATCH_FLAGS
    const SDK_MASK: u32 = Flag::CloseWrite.bits()
        | Flag::Create.bits()
        | Flag::Delete.bits()
        | Flag::MovedTo.bits()
        | Flag::MovedFrom.bits()
        | Flag::DeleteSelf.bits()
        | Flag::MoveSelf.bits();

    fn fd_flags(ino: &INotify) -> Result<Value, Error> {
        let fd = ino.fileno()?;
        // SAFETY: fcntl queries of an open fd.
        let (fd_flags, fl_flags) = unsafe {
            (
                libc::fcntl(fd, libc::F_GETFD),
                libc::fcntl(fd, libc::F_GETFL),
            )
        };
        Ok(json!([
            fd_flags & libc::FD_CLOEXEC != 0,
            fl_flags & libc::O_NONBLOCK != 0
        ]))
    }

    fn path(d: &Path, rest: &[u8]) -> PathBuf {
        let mut bytes = d.as_os_str().as_bytes().to_vec();
        bytes.extend_from_slice(rest);
        PathBuf::from(OsStr::from_bytes(&bytes))
    }

    /// `linux(root)` of the script, with ours.
    fn session(d: &Path) -> Value {
        std::fs::create_dir(d).unwrap();
        let mut cookies = Vec::new();
        let mut steps = Vec::new();
        let mut step = |name: &str, v: Value| steps.push(json!([name, v]));
        let none = |()| Value::Null;

        let mut ino = INotify::new(false, false).unwrap();
        step("fd flags", outcome(fd_flags(&ino), |v| v));
        step(
            "read, nothing, timeout 0",
            outcome(ino.read(Some(0), None), |e| shown(&e, None)),
        );
        step(
            "read, nothing, timeout 50",
            outcome(ino.read(Some(50), None), |e| shown(&e, None)),
        );
        step(
            "read, nothing, timeout 20, delay 5",
            outcome(ino.read(Some(20), Some(5)), |e| shown(&e, None)),
        );
        std::fs::create_dir(path(d, b"/sub")).unwrap();
        let root = ino.add_watch(d, SDK_MASK);
        step("add_watch root", outcome(root, |wd| json!(wd)));
        let sub = ino.add_watch(&path(d, b"/sub"), SDK_MASK);
        let sub_wd = sub.as_ref().ok().copied();
        step("add_watch sub", outcome(sub, |wd| json!(wd)));
        step(
            "add_watch root again",
            outcome(ino.add_watch(d, SDK_MASK), |wd| json!(wd)),
        );
        step(
            "add_watch missing",
            outcome(ino.add_watch(&path(d, b"/missing"), SDK_MASK), |wd| {
                json!(wd)
            }),
        );
        step(
            "add_watch, a NUL in the path",
            outcome(ino.add_watch(&path(d, b"/sub\0ignored"), SDK_MASK), |wd| {
                json!(wd)
            }),
        );

        std::fs::write(path(d, b"/a.gcode"), b"G28\n").unwrap();
        std::fs::rename(path(d, b"/a.gcode"), path(d, b"/sub/b.gcode")).unwrap();
        std::fs::create_dir(path(d, b"/new")).unwrap();
        std::fs::write(path(d, b"/\xff.gcode"), b"M84\n").unwrap();
        std::fs::remove_file(path(d, b"/sub/b.gcode")).unwrap();
        std::fs::remove_dir(path(d, b"/new")).unwrap();
        step(
            "read the changes",
            outcome(ino.read(Some(0), None), |e| shown(&e, Some(&mut cookies))),
        );

        let sub_wd = sub_wd.unwrap();
        step("rm_watch sub", outcome(ino.rm_watch(sub_wd), none));
        step(
            "read, IGNORED",
            outcome(ino.read(Some(1000), None), |e| {
                shown(&e, Some(&mut cookies))
            }),
        );
        step("rm_watch sub again", outcome(ino.rm_watch(sub_wd), none));

        let late = path(d, b"/late.gcode");
        let timer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::write(late, b"G1\n").unwrap();
        });
        step(
            "read, waiting, delay 100",
            outcome(ino.read(Some(5000), Some(100)), |e| shown(&e, None)),
        );
        timer.join().unwrap();

        std::fs::remove_file(path(d, b"/\xff.gcode")).unwrap();
        std::fs::remove_file(path(d, b"/late.gcode")).unwrap();
        std::fs::remove_dir(path(d, b"/sub")).unwrap();
        std::fs::remove_dir(d).unwrap();
        step(
            "read, the root removed",
            outcome(ino.read(Some(1000), Some(50)), |e| {
                shown(&e, Some(&mut cookies))
            }),
        );

        let mut other = INotify::new(true, true).unwrap();
        step(
            "fd flags, inheritable and nonblocking",
            outcome(fd_flags(&other), |v| v),
        );
        step(
            "read nonblocking, nothing",
            outcome(other.read(Some(0), None), |e| shown(&e, None)),
        );
        other.close().unwrap();

        step("close", outcome(ino.close(), none));
        step(
            "read after close",
            outcome(ino.read(Some(0), None), |e| shown(&e, None)),
        );
        step(
            "add_watch after close",
            outcome(ino.add_watch(Path::new("/tmp"), SDK_MASK), |wd| json!(wd)),
        );
        step("rm_watch after close", outcome(ino.rm_watch(1), none));
        step("close again", outcome(ino.close(), none));
        json!({"version": maker_connect_inotify_simple::VERSION, "steps": steps})
    }

    #[test]
    fn session_as_the_reference() {
        let Some(python) = python() else {
            return;
        };
        let base = std::env::temp_dir().join(format!("inotify-simple-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let theirs = base.join("reference");
        let want = reference(&python, &["linux", theirs.to_str().unwrap()]);
        let got = session(&base.join("ours"));
        let _ = std::fs::remove_dir_all(&base);
        let (want, got) = (&want["steps"], &got["steps"]);
        for (w, g) in want.as_array().unwrap().iter().zip(got.as_array().unwrap()) {
            assert_eq!(g, w);
        }
        assert_eq!(got, want);
    }
}
