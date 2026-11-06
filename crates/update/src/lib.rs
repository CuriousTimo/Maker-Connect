//! Updating itself, safely: for the hub and for the agent alike.
//!
//! - **Signed.** A release's tag and its `SHA256SUMS` are signed together (`SHA256SUMS.sig`,
//!   Ed25519, over `<tag>\n<SHA256SUMS>`: an old release cannot pass for a new one) with a key
//!   whose private half only who releases keeps, never in the repository; [`RELEASE_KEY`] is
//!   its public half. A file is taken only when the list's signature holds and the file's
//!   sum is on it ([`check`]): a hacked release page, or a hub that was broken into, cannot
//!   hand out a build of its own.
//! - **Tried first.** The new build is written beside the running one and started once as a
//!   trial (the hub: on a copy of its database; the agent: `--version`). Only a build that
//!   ends its trial well is put in place, the old one kept as `<exe>.previous` ([`install`]).
//! - **A way back.** The old build, which is known to work, stays in charge while the new one
//!   runs ([`supervise`]): it starts the new build as its child, on trial ([`on_start`]), and
//!   waits for it to confirm itself ([`confirm`]: the hub once it serves for a while, the agent
//!   once the hub answered it). A new build that stops, hangs or is not confirmed in time is
//!   stopped and rolled back to the build before ([`roll_back`]), which says so ([`last`]);
//!   even one that cannot start at all. A confirmed one is then started for good.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The public half of the key Maker Connect's releases are signed with
/// (`cargo run -p maker-connect-update --example release-key -- public <key>`).
pub const RELEASE_KEY: [u8; 32] = [
    0x70, 0x53, 0x38, 0x06, 0x91, 0x2e, 0x20, 0x5c, 0x3f, 0x14, 0x61, 0x46, 0x87, 0xb9, 0x4c, 0x01,
    0x31, 0xc8, 0x52, 0xbe, 0xdb, 0x43, 0xb1, 0xb2, 0x65, 0x40, 0xce, 0xf5, 0xc3, 0x58, 0x82, 0x51,
];

/// The project's releases on GitHub, newest first (betas too).
pub const RELEASES: &str =
    "https://api.github.com/repos/CuriousTimo/Maker-Connect/releases?per_page=50";

/// How long a trial run may take before it counts as failed.
pub const TRIAL: Duration = Duration::from_secs(60);

/// How often a new build may start before it counts as falling over.
const MAX_STARTS: u32 = 3;

/// A release's version: `v0.2.0-beta.1`, `hub-v0.2.0`, `0.2.0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
    /// `beta.1`: before the release without it.
    pre: Option<(String, u64)>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        // A tag's part first (`hub-`, `agent-`), then `v`.
        let text = text.rsplit_once("-v").map_or(text, |(_, v)| v);
        let text = text.strip_prefix('v').unwrap_or(text);
        let (numbers, pre) = match text.split_once('-') {
            Some((n, p)) => (n, Some(p)),
            None => (text, None),
        };
        let mut parts = numbers.split('.').map(|n| n.parse::<u64>().ok());
        let numbers = [parts.next()??, parts.next()??, parts.next()??];
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => None,
            Some(p) => {
                let (name, n) = p.split_once('.').unwrap_or((p, "0"));
                Some((name.to_owned(), n.parse().ok()?))
            }
        };
        Some(Version { numbers, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether `candidate` is a newer release than `current`; never when either does not read
/// (a build from the source says `0.2.0`, which does read).
pub fn newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Some(c), Some(now)) => c > now,
        _ => false,
    }
}

/// The part of a release file's name for a system: `linux-armv6` (the 32-bit ARM build,
/// which also runs on ARMv7), `macos-aarch64`, `windows-x86_64`.
pub fn system(os: &str, arch: &str) -> Option<String> {
    let arch = match (os, arch) {
        (_, "x86_64") => "x86_64",
        ("macos" | "linux", "aarch64") => "aarch64",
        ("linux", "arm" | "armv6" | "armv7") => "armv6",
        _ => return None,
    };
    matches!(os, "macos" | "linux" | "windows").then(|| format!("{os}-{arch}"))
}

/// This program's system, as [`system`] names it.
pub fn this_system() -> Option<String> {
    system(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What a release's signature is over: its tag, then its list of sums.
pub fn signed_text(tag: &str, sums: &str) -> String {
    format!("{tag}\n{sums}")
}

/// Whether `signature` (base64) is the release key's over release `tag` with list `sums`.
pub fn verify(tag: &str, sums: &str, signature: &str, key: &[u8]) -> Result<(), String> {
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature.trim())
        .map_err(|_| "the release's signature does not read".to_owned())?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(signed_text(tag, sums).as_bytes(), &signature)
        .map_err(|_| "the release is not signed with Maker Connect's key: refused".to_owned())
}

/// A file's sum on a `SHA256SUMS` list (`<sum>  <name>`, as `shasum -a 256` writes it).
pub fn sum_of<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (sum, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then_some(sum)
    })
}

/// A downloaded file of release `tag`, checked: the release's list is signed with `key`,
/// and the file's sum is on it.
pub fn check(
    bytes: &[u8],
    name: &str,
    tag: &str,
    sums: &str,
    signature: &str,
    key: &[u8],
) -> Result<(), String> {
    verify(tag, sums, signature, key)?;
    let listed = sum_of(sums, name).ok_or_else(|| format!("{name} is not in the release"))?;
    if !listed.eq_ignore_ascii_case(&sha256_hex(bytes)) {
        return Err(format!("{name} is not the file the release lists: refused"));
    }
    Ok(())
}

/// A new build on trial: put in place, not yet confirmed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    pub from: String,
    pub to: String,
    /// Unix seconds: confirmed by then, or rolled back.
    pub until: i64,
    /// How often it started.
    pub starts: u32,
    /// What the program wants to remember of it (the hub: its database's backup).
    #[serde(default)]
    pub note: serde_json::Value,
}

/// How the last update went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub at: i64,
    pub from: String,
    pub to: String,
    pub ok: bool,
    /// Why it was rolled back; empty when it went well.
    pub note: String,
}

fn beside(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn trial_file(exe: &Path) -> PathBuf {
    beside(exe, ".update.json")
}

fn outcome_file(exe: &Path) -> PathBuf {
    beside(exe, ".update-result.json")
}

/// The build before the one at `exe`, kept for the way back.
pub fn previous(exe: &Path) -> PathBuf {
    beside(exe, ".previous")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs `program` once with `args`: it must end well within [`TRIAL`].
fn try_out(program: &Path, args: &[OsString]) -> Result<(), String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("the new build does not start here: {e}"))?;
    let until = std::time::Instant::now() + TRIAL;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("the new build failed its trial ({status})")),
            Ok(None) if std::time::Instant::now() > until => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the new build did not end its trial in {} s",
                    TRIAL.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Puts `build` in place of the program at `exe`, after a trial run with `trial_args`; the
/// old build is kept as [`previous`]. The new one is then on trial: it must [`confirm`]
/// itself within `grace_s` seconds of its start. Then [`restart`] it.
pub fn install(
    exe: &Path,
    build: &[u8],
    trial_args: &[OsString],
    from: &str,
    to: &str,
    grace_s: i64,
    note: serde_json::Value,
) -> Result<(), String> {
    let new = beside(exe, ".new");
    std::fs::write(&new, build).map_err(|e| format!("{}: {e}", new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755));
    }
    if let Err(error) = try_out(&new, trial_args) {
        let _ = std::fs::remove_file(&new);
        return Err(error);
    }
    let kept = previous(exe);
    let _ = std::fs::remove_file(&kept);
    std::fs::rename(exe, &kept).map_err(|e| format!("could not set the old build aside: {e}"))?;
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&kept, exe);
        return Err(format!("could not put the new build in place: {e}"));
    }
    write_json(
        &trial_file(exe),
        &Trial {
            from: from.to_owned(),
            to: to.to_owned(),
            until: now() + grace_s,
            starts: 0,
            note,
        },
    )
}

/// How this start goes.
#[derive(Debug, PartialEq)]
pub enum Start {
    /// No update under way.
    Plain,
    /// A new build on trial: [`confirm`] it once it works.
    OnTrial(Trial),
    /// A new build that fell over or was not confirmed in time: [`roll_back`], then
    /// [`restart`].
    RollBack(Trial, String),
}

/// At the start of the program at `exe`: whether a new build is on trial; counts this start.
pub fn on_start(exe: &Path) -> Start {
    let path = trial_file(exe);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Start::Plain;
    };
    let Ok(mut trial) = serde_json::from_str::<Trial>(&text) else {
        let _ = std::fs::remove_file(&path);
        return Start::Plain;
    };
    trial.starts += 1;
    let _ = write_json(&path, &trial);
    if trial.starts > MAX_STARTS {
        Start::RollBack(trial, format!("it stopped {MAX_STARTS} times"))
    } else if now() > trial.until {
        Start::RollBack(trial, "it did not work in time".to_owned())
    } else {
        Start::OnTrial(trial)
    }
}

/// The new build works: no way back needed any more.
pub fn confirm(exe: &Path, trial: &Trial) {
    let _ = std::fs::remove_file(trial_file(exe));
    let _ = write_json(
        &outcome_file(exe),
        &Outcome {
            at: now(),
            from: trial.from.clone(),
            to: trial.to.clone(),
            ok: true,
            note: String::new(),
        },
    );
}

/// An update that did not get as far as a trial (a download refused, a trial failed): noted
/// as its outcome, as a rolled-back one is.
pub fn note_failure(exe: &Path, from: &str, to: &str, why: &str) {
    let _ = write_json(
        &outcome_file(exe),
        &Outcome {
            at: now(),
            from: from.to_owned(),
            to: to.to_owned(),
            ok: false,
            note: why.to_owned(),
        },
    );
}

/// How a new build on trial went, as [`supervise`] saw it.
#[derive(Debug, PartialEq)]
pub enum Supervised {
    /// It confirmed itself: start it for good ([`restart`]).
    Confirmed,
    /// It stopped, or did not confirm itself in time: [`roll_back`], then [`restart`].
    Failed(Trial, String),
}

/// Runs the new build at `exe` (put in place by [`install`]) as a child, with `args`, while
/// this process (the build before, known to work) waits: until the child confirms itself,
/// stops, or its time is up. The child is stopped either way: a confirmed build is then
/// started for good, in this one's place.
pub fn supervise(exe: &Path, args: &[OsString]) -> Result<Supervised, String> {
    let trial: Trial = serde_json::from_str(
        &std::fs::read_to_string(trial_file(exe)).map_err(|_| "no update under way".to_owned())?,
    )
    .map_err(|e| e.to_string())?;
    let mut child = match std::process::Command::new(exe).args(args).spawn() {
        Ok(child) => child,
        Err(e) => return Ok(Supervised::Failed(trial, format!("it does not start: {e}"))),
    };
    let stop = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    loop {
        if !trial_file(exe).is_file() {
            stop(&mut child);
            return Ok(Supervised::Confirmed);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Supervised::Failed(trial, format!("it stopped ({status})")));
            }
            Ok(None) if now() > trial.until => {
                stop(&mut child);
                return Ok(Supervised::Failed(
                    trial,
                    "it did not work in time".to_owned(),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(300)),
            Err(e) => {
                stop(&mut child);
                return Ok(Supervised::Failed(trial, e.to_string()));
            }
        }
    }
}

/// Whether a trial is still waiting for its [`confirm`] (a timer checks it).
pub fn still_on_trial(exe: &Path) -> bool {
    trial_file(exe).is_file()
}

/// Puts the build before back in place (the failed one kept as `<exe>.failed`) and notes
/// why; then [`restart`].
pub fn roll_back(exe: &Path, trial: &Trial, why: &str) -> Result<(), String> {
    let kept = previous(exe);
    if !kept.is_file() {
        let _ = std::fs::remove_file(trial_file(exe));
        return Err(format!("no build before {} to go back to", trial.to));
    }
    let failed = beside(exe, ".failed");
    let _ = std::fs::remove_file(&failed);
    std::fs::rename(exe, &failed).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&kept, exe) {
        let _ = std::fs::rename(&failed, exe);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_file(trial_file(exe));
    write_json(
        &outcome_file(exe),
        &Outcome {
            at: now(),
            from: trial.from.clone(),
            to: trial.to.clone(),
            ok: false,
            note: why.to_owned(),
        },
    )
}

/// How the last update of the program at `exe` went.
pub fn last(exe: &Path) -> Option<Outcome> {
    serde_json::from_str(&std::fs::read_to_string(outcome_file(exe)).ok()?).ok()
}

/// Starts the program at `exe` again with `args`, in place of this one: on Linux and macOS
/// the same process (systemd and a terminal keep it), on Windows a new one.
pub fn restart(exe: &Path, args: &[OsString]) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = std::process::Command::new(exe).args(args).exec();
        eprintln!("could not start {}: {error}", exe.display());
        std::process::exit(1)
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(exe).args(args).spawn() {
            Ok(_) => std::process::exit(0),
            Err(error) => {
                eprintln!("could not start {}: {error}", exe.display());
                std::process::exit(1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;

    #[test]
    fn versions_are_ordered_as_releases() {
        assert!(newer("hub-v0.2.0-beta.2", "v0.2.0-beta.1"));
        assert!(newer("v0.2.0", "0.2.0-beta.9"));
        assert!(newer("agent-v0.10.0", "v0.9.3"));
        assert!(!newer("v0.2.0-beta.1", "v0.2.0-beta.1"));
        assert!(
            !newer("v0.1.0", "0.2.0"),
            "a build from the source is 0.2.0"
        );
        assert!(!newer("nonsense", "v0.1.0"));
        assert_eq!(system("linux", "arm").as_deref(), Some("linux-armv6"));
        assert_eq!(system("windows", "aarch64"), None);
    }

    #[test]
    fn only_a_signed_file_on_the_list_is_taken() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let build = b"a new build";
        let sums = format!("{}  maker-connect-agent-linux-armv6\n", sha256_hex(build));
        let tag = "agent-v0.3.0";
        let signature = base64::engine::general_purpose::STANDARD
            .encode(pair.sign(signed_text(tag, &sums).as_bytes()));
        let key = pair.public_key().as_ref();
        let name = "maker-connect-agent-linux-armv6";
        assert_eq!(check(build, name, tag, &sums, &signature, key), Ok(()));
        assert!(check(b"another", name, tag, &sums, &signature, key).is_err());
        assert!(check(build, "other", tag, &sums, &signature, key).is_err());
        assert!(
            check(build, name, "agent-v9.0.0", &sums, &signature, key).is_err(),
            "an old release does not pass for a newer one"
        );
        let forged = sums.replace("armv6", "armv7");
        let armv7 = "maker-connect-agent-linux-armv7";
        assert!(check(build, armv7, tag, &forged, &signature, key).is_err());
        assert!(
            verify(tag, &sums, &signature, &RELEASE_KEY).is_err(),
            "not with another key"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_build_is_tried_put_in_place_and_can_go_back() {
        let dir =
            std::env::temp_dir().join(format!("mc-update-{}", now() ^ std::process::id() as i64));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("program");
        std::fs::write(&exe, "#!/bin/sh\necho old\n").unwrap();
        // A build that fails its trial changes nothing.
        let broken = b"#!/bin/sh\nexit 3\n";
        let error = install(
            &exe,
            broken,
            &[],
            "v1.0.0",
            "v1.1.0",
            60,
            serde_json::Value::Null,
        );
        assert!(error.unwrap_err().contains("failed its trial"));
        assert_eq!(
            std::fs::read_to_string(&exe).unwrap(),
            "#!/bin/sh\necho old\n"
        );
        assert_eq!(on_start(&exe), Start::Plain);

        let good = b"#!/bin/sh\nexit 0\n";
        install(
            &exe,
            good,
            &[],
            "v1.0.0",
            "v1.1.0",
            60,
            serde_json::json!({"db": "x"}),
        )
        .unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), good);
        assert!(previous(&exe).is_file());
        // Started three times: still on trial; the fourth time, it goes back.
        for _ in 0..3 {
            assert!(matches!(on_start(&exe), Start::OnTrial(_)));
        }
        let Start::RollBack(trial, why) = on_start(&exe) else {
            panic!("a fourth start rolls back")
        };
        assert_eq!(trial.note["db"], "x");
        roll_back(&exe, &trial, &why).unwrap();
        assert_eq!(
            std::fs::read_to_string(&exe).unwrap(),
            "#!/bin/sh\necho old\n"
        );
        let outcome = last(&exe).unwrap();
        assert!(!outcome.ok && outcome.to == "v1.1.0" && outcome.note.contains("3 times"));
        assert_eq!(on_start(&exe), Start::Plain);

        // Confirmed in time: it stays.
        install(
            &exe,
            good,
            &[],
            "v1.0.0",
            "v1.1.0",
            60,
            serde_json::Value::Null,
        )
        .unwrap();
        let Start::OnTrial(trial) = on_start(&exe) else {
            panic!()
        };
        confirm(&exe, &trial);
        assert!(!still_on_trial(&exe));
        assert!(last(&exe).unwrap().ok);
        assert_eq!(on_start(&exe), Start::Plain);
        assert_eq!(std::fs::read(&exe).unwrap(), good);

        // Watched by the build before: one that confirms itself (here: removes its trial
        // file, as `confirm` does) is kept; one that stops, or hangs past its time, is not.
        let confirms =
            b"#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nrm -f \"$0.update.json\"\nsleep 30\n";
        let version: Vec<OsString> = vec!["--version".into()];
        install(
            &exe,
            confirms,
            &version,
            "v1.1.0",
            "v1.2.0",
            30,
            serde_json::Value::Null,
        )
        .unwrap();
        assert_eq!(supervise(&exe, &[]), Ok(Supervised::Confirmed));
        let stops = b"#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nexit 7\n";
        install(
            &exe,
            stops,
            &version,
            "v1.2.0",
            "v1.3.0",
            30,
            serde_json::Value::Null,
        )
        .unwrap();
        let Ok(Supervised::Failed(trial, why)) = supervise(&exe, &[]) else {
            panic!()
        };
        assert!(why.contains("stopped"), "{why}");
        roll_back(&exe, &trial, &why).unwrap();
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            confirms,
            "the build before is back"
        );
        let hangs = b"#!/bin/sh\n[ \"$1\" = --version ] && exit 0\nsleep 30\n";
        install(
            &exe,
            hangs,
            &version,
            "v1.2.0",
            "v1.3.0",
            1,
            serde_json::Value::Null,
        )
        .unwrap();
        let Ok(Supervised::Failed(_, why)) = supervise(&exe, &[]) else {
            panic!()
        };
        assert!(why.contains("in time"), "{why}");
        let _ = std::fs::remove_file(trial_file(&exe));

        // Not confirmed in time: back.
        install(
            &exe,
            b"#!/bin/sh\nexit 0\n#2\n",
            &[],
            "v1.1.0",
            "v1.2.0",
            -1,
            serde_json::Value::Null,
        )
        .unwrap();
        assert!(matches!(on_start(&exe), Start::RollBack(_, why) if why.contains("in time")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
