//! Bakes the build's identity into the agent: the git commit it was built from (with
//! `-dirty` when files differed from it) and when, so every agent can say exactly what it is
//! (`--version`, at start, and to the hub). `scripts/build-downloads.sh` writes the same
//! commit beside the builds it puts in the hub's downloads.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Days since 1970-01-01 as a date (Howard Hinnant's civil-from-days).
fn date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn main() {
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = date(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    println!(
        "cargo:rustc-env=MC_COMMIT={commit}{}",
        if dirty { "-dirty" } else { "" }
    );
    println!(
        "cargo:rustc-env=MC_BUILT={y}-{m:02}-{d:02} {:02}:{:02} UTC",
        t / 3600,
        t / 60 % 60
    );
    // Built again when the commit changes, or what is staged.
    for path in [
        "../../.git/HEAD",
        "../../.git/index",
        "../../.git/refs/heads",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=MAKER_CONNECT_RELEASE");
}
