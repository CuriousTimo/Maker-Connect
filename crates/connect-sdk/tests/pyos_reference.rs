//! Ours (`src/pyos.rs`) against CPython run on the same steps: `tests/pyos_scenario.py` runs the
//! reference and prints what it gave; this test runs ours the same way and wants the same.
//!
//! The Python, CPython 3.13.12 (the reference; checked): `PYOS_PYTHON` (CI's job `store`), else
//! `reference/.venv-connect-sdk-313`. Without one it says so and passes.
//! One test, as it sets the process's cwd.

#![cfg(unix)]

use maker_connect_sdk::pyos::{self, DirEntry, Error, Filename, StatResult};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn python() -> Option<String> {
    if let Ok(p) = std::env::var("PYOS_PYTHON") {
        return Some(p);
    }
    let venv = format!(
        "{}/../../reference/.venv-connect-sdk-313/bin/python",
        env!("CARGO_MANIFEST_DIR")
    );
    if Path::new(&venv).exists() {
        return Some(venv);
    }
    // CI's job `references` sets REFERENCES_REQUIRED: there a skip is a failure.
    assert!(
        std::env::var_os("REFERENCES_REQUIRED").is_none(),
        "REFERENCES_REQUIRED: skipped: no CPython 3.13.12 (PYOS_PYTHON)"
    );
    eprintln!("skipped: no CPython 3.13.12 (PYOS_PYTHON)");
    None
}

fn run(python: &str, cwd: &Path, args: &[&str]) -> Value {
    let script = format!("{}/tests/pyos_scenario.py", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(python)
        .arg("-I")
        .arg(script)
        .args(args)
        .current_dir(cwd)
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

fn h(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// `strings(max_len)` of the script: `itertools.product` over "/.a", shortest first.
fn strings(max_len: usize) -> Vec<Vec<u8>> {
    let mut out = vec![Vec::new()];
    let mut last = vec![Vec::new()];
    for _ in 0..max_len {
        let mut next = Vec::new();
        for s in &last {
            for c in b"/.a" {
                let mut t = s.clone();
                t.push(*c);
                next.push(t);
            }
        }
        out.extend(next.iter().cloned());
        last = next;
    }
    out
}

struct Shown {
    base: Option<Vec<u8>>,
}

impl Shown {
    fn path(&self, p: &[u8]) -> Value {
        if let Some(base) = &self.base
            && p.starts_with(base)
        {
            let mut b = b"<base>".to_vec();
            b.extend_from_slice(&p[base.len()..]);
            return json!(h(&b));
        }
        json!(h(p))
    }

    fn filename(&self, f: &Option<Filename>) -> Value {
        match f {
            None => Value::Null,
            Some(Filename::Fd(fd)) => json!(fd),
            Some(Filename::Path(p)) => self.path(p),
        }
    }

    fn outcome<T>(&self, r: Result<T, Error>, ok: impl FnOnce(T) -> Value) -> Value {
        match r {
            Ok(v) => json!(["ok", ok(v)]),
            Err(Error::Os(err)) => json!([err.repr(), self.filename(&err.filename)]),
            Err(err @ Error::Value(_)) => json!([err.repr(), Value::Null]),
        }
    }
}

fn hv(b: Vec<u8>) -> Value {
    json!(h(&b))
}

fn pure() -> Value {
    let sh = Shown { base: None };
    let s6 = strings(6);
    let s3 = strings(3);
    let s2 = strings(2);
    let s4 = strings(4);
    let mut join2 = Vec::new();
    for a in &s3 {
        for b in &s3 {
            join2.push(hv(pyos::join(a, &[b])));
        }
    }
    let mut join3 = Vec::new();
    for a in &s2 {
        for b in &s2 {
            for c in &s2 {
                join3.push(hv(pyos::join(a, &[b, c])));
            }
        }
    }
    let mut cp2 = Vec::new();
    for a in &s4 {
        for b in &s4 {
            cp2.push(sh.outcome(pyos::commonpath(&[a, b]), hv));
        }
    }
    let mut cp3 = Vec::new();
    for a in &s2 {
        for b in &s2 {
            for c in &s2 {
                cp3.push(sh.outcome(pyos::commonpath(&[a, b, c]), hv));
            }
        }
    }
    let mut rel = Vec::new();
    for a in &s4 {
        for b in &s4 {
            rel.push(sh.outcome(pyos::relpath(a, Some(b)), hv));
        }
    }
    let nul = b"a\0b";
    json!({
        "normpath": s6.iter().map(|p| hv(pyos::normpath(p))).collect::<Vec<_>>(),
        "split": s6.iter().map(|p| { let (a, b) = pyos::split(p); json!([h(&a), h(&b)]) }).collect::<Vec<_>>(),
        "join2": join2,
        "join3": join3,
        "join1": s3.iter().map(|a| hv(pyos::join(a, &[]))).collect::<Vec<_>>(),
        "commonpath1": s4.iter().map(|a| sh.outcome(pyos::commonpath(&[a]), hv)).collect::<Vec<_>>(),
        "commonpath2": cp2,
        "commonpath3": cp3,
        "commonpath0": sh.outcome(pyos::commonpath(&[]), hv),
        "abspath": s4.iter().map(|a| sh.outcome(pyos::abspath(a), hv)).collect::<Vec<_>>(),
        "relpath": rel,
        "relpath_nostart": s4.iter().map(|a| sh.outcome(pyos::relpath(a, None), hv)).collect::<Vec<_>>(),
        "nul": [
            sh.outcome(Ok(pyos::normpath(b"a/\0/../b")), hv),
            sh.outcome(pyos::stat(nul), |_| Value::Null),
            [pyos::exists(nul), pyos::lexists(nul), pyos::isdir(nul), pyos::islink(nul)],
            sh.outcome(pyos::realpath(b"/tmp/a\0b"), hv),
        ],
    })
}

fn st_shown(st: StatResult) -> Value {
    json!([st.st_mode, st.st_size, st.st_mtime as i64])
}

fn entry_shown(sh: &Shown, e: &mut DirEntry) -> Value {
    json!([
        h(&e.name),
        sh.path(&e.path.clone()),
        sh.outcome(e.is_dir(true), |v| json!(v)),
        sh.outcome(e.is_dir(false), |v| json!(v)),
        sh.outcome(e.is_symlink(), |v| json!(v)),
        sh.outcome(e.stat(true), st_shown),
        sh.outcome(e.stat(false), st_shown),
    ])
}

fn scandir_shown(sh: &Shown, p: &[u8]) -> Value {
    let r = pyos::scandir(p).and_then(|it| {
        let mut out = Vec::new();
        for e in it {
            out.push(entry_shown(sh, &mut e?));
        }
        Ok(out)
    });
    sh.outcome(r, Value::Array)
}

fn walk_shown(
    sh: &Shown,
    top: &[u8],
    topdown: bool,
    followlinks: bool,
    with_onerror: bool,
) -> Value {
    use std::cell::RefCell;
    use std::rc::Rc;
    let errors = Rc::new(RefCell::new(Vec::new()));
    let onerror: Option<pyos::OnError> = with_onerror.then(|| {
        let errors = errors.clone();
        let base = sh.base.clone();
        Box::new(move |err: Error| {
            let sh = Shown { base: base.clone() };
            let Error::Os(e) = &err else { unreachable!() };
            errors
                .borrow_mut()
                .push(json!([e.repr(), sh.filename(&e.filename)]));
            Ok(())
        }) as pyos::OnError
    });
    let out: Vec<Value> = pyos::walk(top, topdown, onerror, followlinks)
        .map(|item| {
            let (d, dirs, files) = item.unwrap();
            json!([
                sh.path(&d),
                dirs.iter().map(|n| h(n)).collect::<Vec<_>>(),
                files.iter().map(|n| h(n)).collect::<Vec<_>>()
            ])
        })
        .collect();
    let errors = errors.borrow().clone();
    json!([out, errors])
}

fn bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

fn cat(a: &[u8], b: &[u8]) -> Vec<u8> {
    [a, b].concat()
}

fn listdir_sorted(p: &Path) -> Vec<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let mut names: Vec<Vec<u8>> = std::fs::read_dir(p)
        .unwrap()
        .map(|e| e.unwrap().file_name().as_bytes().to_vec())
        .collect();
    // sorted(os.listdir(...)) of str names: these are ASCII, so bytes order is the same.
    names.sort();
    names
}

fn fs(root: &Path) -> Value {
    let rootb = bytes(root);
    let sh = Shown {
        base: Some(rootb.clone()),
    };
    let w = pyos::join(&rootb, &[b"w"]);
    let r = pyos::join(&rootb, &[b"r"]);
    let mut names_w = listdir_sorted(&root.join("w"));
    for extra in [
        &b"missing"[..],
        b"sub/deep",
        b"sub/.hid/d.gcode",
        b"noperm/x",
    ] {
        names_w.push(extra.to_vec());
    }
    let names_r = listdir_sorted(&root.join("r"));
    let suffixes: [&[u8]; 8] = [
        b"",
        b"/",
        b"/.",
        b"/..",
        b"/x",
        b"/../sub",
        b"/./c.txt",
        b"//deep",
    ];
    let mut paths: Vec<Vec<u8>> = Vec::new();
    for n in &names_w {
        for s in suffixes {
            paths.push(cat(&pyos::join(&w, &[n]), s));
        }
    }
    for n in &names_r {
        for s in suffixes {
            paths.push(cat(&pyos::join(&r, &[n]), s));
        }
    }
    for n in &names_r {
        for s in suffixes {
            paths.push(cat(n, s));
        }
    }
    for p in [
        &b""[..],
        b".",
        b"..",
        b"/",
        b"//",
        b"///",
        b"/..",
        b"../w/./sub",
        b"up/up/r/self",
    ] {
        paths.push(p.to_vec());
    }
    let mut steps = Vec::new();
    let mut step = |name: String, v: Value| steps.push(json!([name, v]));
    let path_v = |p: Vec<u8>| sh.path(&p);

    step(
        "realpath".into(),
        paths
            .iter()
            .map(|p| sh.outcome(pyos::realpath(p), path_v))
            .collect(),
    );
    step(
        "stat".into(),
        paths
            .iter()
            .map(|p| sh.outcome(pyos::stat(p), st_shown))
            .collect(),
    );
    step(
        "lstat".into(),
        paths
            .iter()
            .map(|p| sh.outcome(pyos::lstat(p), st_shown))
            .collect(),
    );
    step(
        "tests".into(),
        paths
            .iter()
            .map(|p| {
                json!([
                    pyos::exists(p),
                    pyos::lexists(p),
                    pyos::isdir(p),
                    pyos::islink(p),
                    pyos::access(p, pyos::W_OK).unwrap()
                ])
            })
            .collect(),
    );
    step(
        "abspath".into(),
        paths
            .iter()
            .map(|p| sh.outcome(pyos::abspath(p), path_v))
            .collect(),
    );
    step(
        "relpath".into(),
        paths
            .iter()
            .map(|p| sh.outcome(pyos::relpath(p, Some(&w)), path_v))
            .collect(),
    );
    step(
        "statvfs /dev".into(),
        sh.outcome(pyos::statvfs(b"/dev"), |s| {
            json!([s.f_bavail * s.f_bsize, s.f_blocks * s.f_bsize])
        }),
    );
    step(
        "statvfs root".into(),
        sh.outcome(pyos::statvfs(&rootb), |s| json!([s.f_blocks * s.f_bsize])),
    );
    // Ours counts in `f_frsize` blocks, the SDK in `f_bsize` ones: the same except on macOS.
    step(
        "statvfs root, f_bsize is f_frsize".into(),
        json!(!cfg!(target_os = "macos")),
    );
    step(
        "statvfs missing".into(),
        sh.outcome(pyos::statvfs(&pyos::join(&w, &[b"missing"])), |_| {
            Value::Null
        }),
    );
    step("scandir w".into(), scandir_shown(&sh, &w));
    step("scandir w/".into(), scandir_shown(&sh, &cat(&w, b"/")));
    step("scandir r".into(), scandir_shown(&sh, &r));
    let tops = [
        w.clone(),
        cat(&w, b"/"),
        b"../w".to_vec(),
        pyos::join(&w, &[b"sub"]),
        pyos::join(&w, &[b"missing"]),
        pyos::join(&w, &[b"a.gcode"]),
        pyos::join(&w, &[b"noperm"]),
        pyos::join(&w, &[b"link_sub"]),
    ];
    let py_bool = |b: bool| if b { "True" } else { "False" };
    for top in &tops {
        for topdown in [true, false] {
            for followlinks in [false, true] {
                for with_onerror in [false, true] {
                    let shown = sh.path(top);
                    step(
                        format!(
                            "walk {} {} {} {}",
                            shown.as_str().unwrap(),
                            py_bool(topdown),
                            py_bool(followlinks),
                            py_bool(with_onerror)
                        ),
                        walk_shown(&sh, top, topdown, followlinks, with_onerror),
                    );
                }
            }
        }
    }
    for p in [
        &b"sub"[..],
        b"missing",
        b"a.gcode",
        b"noperm/x",
        b"link_sub",
    ] {
        let p = pyos::join(&w, &[p]);
        step(
            format!("rmdir {}", sh.path(&p).as_str().unwrap()),
            sh.outcome(pyos::rmdir(&p), |_| Value::Null),
        );
    }
    for p in [&b"sub"[..], b"missing", b"noperm/x", b"sub/deep/c.txt/x"] {
        let p = pyos::join(&w, &[p]);
        step(
            format!("unlink {}", sh.path(&p).as_str().unwrap()),
            sh.outcome(pyos::unlink(&p), |_| Value::Null),
        );
    }
    json!(steps)
}

const RMTREE_CASES: [&str; 7] = [
    "tree",
    "link_to_dir",
    "missing",
    "file",
    "noperm_sub",
    "empty",
    "readonly",
];

fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// `made(d)` of the script: every path under `d`, sorted.
fn made(d: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for item in pyos::walk(d, true, None, false) {
        let (top, dirs, files) = item.unwrap();
        for n in dirs.iter().chain(files.iter()) {
            out.push(pyos::join(&top, &[n]));
        }
    }
    out.sort();
    out
}

fn rmtree(d: &Path) -> Value {
    let db = bytes(d);
    let sh = Shown {
        base: Some(db.clone()),
    };
    chmod(&d.join("noperm_sub/locked"), 0o755);
    let before = made(&db);
    chmod(&d.join("noperm_sub/locked"), 0);
    let mut steps = Vec::new();
    for case in RMTREE_CASES {
        let p = pyos::join(&db, &[case.as_bytes()]);
        steps.push(json!([case, sh.outcome(pyos::rmtree(&p), |_| Value::Null)]));
    }
    chmod(&d.join("noperm_sub/locked"), 0o755);
    chmod(&d.join("readonly/inner"), 0o755);
    let left: Vec<Value> = before
        .iter()
        .filter(|p| pyos::lexists(p))
        .map(|p| sh.path(p))
        .collect();
    steps.push(json!(["left", left]));
    json!(steps)
}

/// Restores the modes the scenario took away, so the directory can be removed.
fn cleanup(base: &Path) {
    for p in [
        "fs/w/noperm",
        "rmtree-ref/noperm_sub/locked",
        "rmtree-ours/noperm_sub/locked",
        "rmtree-ref/readonly/inner",
        "rmtree-ours/readonly/inner",
    ] {
        let p = base.join(p);
        if p.exists() {
            chmod(&p, 0o755);
        }
    }
    let _ = std::fs::remove_dir_all(base);
}

/// Every step that differs, then a failure if any did.
fn compare(what: &str, ours: &Value, want: &Value) {
    let (o, w) = (ours.as_array().unwrap(), want.as_array().unwrap());
    let differ: Vec<String> = o
        .iter()
        .zip(w)
        .filter(|(o, w)| o != w)
        .map(|(o, w)| format!("  ours: {o}\n  reference: {w}"))
        .collect();
    assert!(differ.is_empty(), "{what}:\n{}", differ.join("\n"));
    assert_eq!(o.len(), w.len(), "{what}: the number of steps");
}

#[test]
fn as_the_reference() {
    let Some(python) = python() else {
        return;
    };
    let base: PathBuf = std::env::temp_dir().join(format!("pyos-test-{}", std::process::id()));
    cleanup(&base);
    std::fs::create_dir_all(&base).unwrap();

    // The string functions, abspath and relpath from the same cwd.
    std::env::set_current_dir(&base).unwrap();
    let mut want = run(&python, &base, &["pure"]);
    let version = want["version"].take();
    assert_eq!(version, "3.13.12", "the reference's version");
    let want_obj = want.as_object_mut().unwrap();
    want_obj.remove("version");
    let ours = pure();
    for (k, v) in want_obj.iter() {
        assert_eq!(&ours[k], v, "pure: {k}");
    }
    assert_eq!(ours.as_object().unwrap().len(), want_obj.len());

    // The fs steps on one tree, the reference first: they do not change it.
    let root = base.join("fs");
    std::fs::create_dir(&root).unwrap();
    run(&python, &base, &["build", root.to_str().unwrap()]);
    let r = root.join("r");
    let want = run(&python, &r, &["fs", root.to_str().unwrap()]);
    std::env::set_current_dir(&r).unwrap();
    let ours = fs(&root);
    compare("fs", &ours, &want["steps"]);

    // rmtree on two copies of the cases.
    let (theirs, mine) = (base.join("rmtree-ref"), base.join("rmtree-ours"));
    run(&python, &base, &["build-rmtree", theirs.to_str().unwrap()]);
    run(&python, &base, &["build-rmtree", mine.to_str().unwrap()]);
    let want = run(&python, &base, &["rmtree", theirs.to_str().unwrap()]);
    let ours = rmtree(&mine);
    compare("rmtree", &ours, &want["steps"]);

    std::env::set_current_dir(std::env::temp_dir()).unwrap();
    cleanup(&base);
    eprintln!("compared with CPython {version}");
}
