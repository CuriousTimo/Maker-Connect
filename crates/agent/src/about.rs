//! The About tab: the agent and the computer it runs on (ours, UI only). Everything is read from
//! Linux's `/proc` and `/sys` when the page is asked for; what cannot be read (another OS, a file
//! that is not there) is left out.

use std::path::Path;

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(Path::new(path)).ok()
}

/// A value of a `key: value` / `key=value` file (`/proc/cpuinfo`, `/proc/meminfo`,
/// `/etc/os-release`): the first line with that key.
fn field(text: &str, key: &str, sep: char) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once(sep)?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
    })
}

/// `MemTotal:  1015636 kB` → MiB.
fn kib_field(text: &str, key: &str) -> Option<u64> {
    field(text, key, ':')?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kib| kib / 1024)
}

fn duration(seconds: u64) -> String {
    let (d, h, m) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        _ => format!("{d} d {h} h"),
    }
}

/// The agent itself: its version, its memory, how long it runs.
pub fn agent() -> Vec<(&'static str, String)> {
    let mut out = vec![("Version", crate::VERSION.to_owned())];
    if let Some(status) = read("/proc/self/status")
        && let Some(rss) = kib_field(&status, "VmRSS")
    {
        out.push(("Memory", format!("{rss} MiB")));
    }
    if let (Some(stat), Some(uptime)) = (read("/proc/self/stat"), read("/proc/uptime"))
        && let Some(started) = process_age(&stat, &uptime)
    {
        out.push(("Running for", duration(started)));
    }
    out
}

/// Seconds since the process started: `/proc/self/stat` field 22 (clock ticks after boot, at
/// 100 a second as on Linux's usual `USER_HZ`), against `/proc/uptime`.
fn process_age(stat: &str, uptime: &str) -> Option<u64> {
    // the fields after the command name, which is in parentheses and may hold spaces
    let rest = &stat[stat.rfind(')')? + 2..];
    let ticks: u64 = rest.split_whitespace().nth(19)?.parse().ok()?;
    let up: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    Some((up - ticks as f64 / 100.0).max(0.0) as u64)
}

/// The computer: its system, kernel, hardware and processor.
pub fn system() -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(name) = read("/etc/os-release").and_then(|t| field(&t, "PRETTY_NAME", '=')) {
        out.push(("System", name));
    }
    if let Some(kernel) = read("/proc/sys/kernel/osrelease") {
        out.push(("Kernel", kernel.trim().to_owned()));
    }
    out.push(("Architecture", std::env::consts::ARCH.to_owned()));
    let model = read("/sys/firmware/devicetree/base/model")
        .map(|m| m.trim_end_matches('\0').trim().to_owned())
        .or_else(|| read("/sys/class/dmi/id/product_name").map(|m| m.trim().to_owned()))
        .filter(|m| !m.is_empty());
    if let Some(model) = model {
        out.push(("Hardware", model));
    }
    if let Some(cpuinfo) = read("/proc/cpuinfo") {
        if let Some(cpu) = cpu_name(&cpuinfo) {
            out.push(("Processor", cpu));
        }
        let cores = cpuinfo
            .lines()
            .filter(|l| l.starts_with("processor"))
            .count();
        if cores > 0 {
            out.push(("Cores", cores.to_string()));
        }
    }
    if let Some(meminfo) = read("/proc/meminfo")
        && let Some(total) = kib_field(&meminfo, "MemTotal")
    {
        out.push(("Memory", format!("{total} MiB")));
    }
    if let Some(name) = read("/proc/sys/kernel/hostname") {
        out.push(("Host name", name.trim().to_owned()));
    }
    out
}

/// The processor's name: `model name` (x86, and many ARM kernels), else `Hardware` (older ARM
/// kernels), else `Model`.
fn cpu_name(cpuinfo: &str) -> Option<String> {
    ["model name", "Hardware", "Model"]
        .iter()
        .find_map(|k| field(cpuinfo, k, ':'))
        .filter(|v| !v.is_empty())
}

/// What the computer does now: processor load and temperature, memory in use, uptime.
pub fn load() -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(load) = read("/proc/loadavg") {
        let avg: Vec<&str> = load.split_whitespace().take(3).collect();
        if avg.len() == 3 {
            out.push((
                "Load",
                format!("{} · {} · {} (1, 5, 15 min)", avg[0], avg[1], avg[2]),
            ));
        }
    }
    if let Some(milli) =
        read("/sys/class/thermal/thermal_zone0/temp").and_then(|t| t.trim().parse::<i64>().ok())
    {
        out.push(("Temperature", format!("{:.1} °C", milli as f64 / 1000.0)));
    }
    if let Some(meminfo) = read("/proc/meminfo")
        && let (Some(total), Some(available)) = (
            kib_field(&meminfo, "MemTotal"),
            kib_field(&meminfo, "MemAvailable"),
        )
    {
        let used = total.saturating_sub(available);
        out.push((
            "Memory in use",
            format!("{used} of {total} MiB ({}%)", used * 100 / total.max(1)),
        ));
    }
    if let Some(up) =
        read("/proc/uptime").and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok())
    {
        out.push(("Up for", duration(up as u64)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_as_linux_writes_them() {
        let meminfo = "MemTotal:        1015636 kB\nMemFree:           68476 kB\nMemAvailable:     576512 kB\n";
        assert_eq!(kib_field(meminfo, "MemTotal"), Some(991));
        assert_eq!(kib_field(meminfo, "MemAvailable"), Some(563));
        let os = "PRETTY_NAME=\"Armbian 23.8.1 bullseye\"\nNAME=\"Debian GNU/Linux\"\n";
        assert_eq!(
            field(os, "PRETTY_NAME", '=').as_deref(),
            Some("Armbian 23.8.1 bullseye")
        );
        // an Allwinner H3 on an older ARM kernel: no `model name`, a `Hardware` line
        let arm = "processor\t: 0\nBogoMIPS\t: 48.00\n\nprocessor\t: 1\nHardware\t: Allwinner sun8i Family\n";
        assert_eq!(cpu_name(arm).as_deref(), Some("Allwinner sun8i Family"));
        let x86 = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i5\n";
        assert_eq!(cpu_name(x86).as_deref(), Some("Intel(R) Core(TM) i5"));
    }

    #[test]
    fn how_long_a_process_runs() {
        // field 22 (starttime) is 1000 ticks: 10 s after boot; up 3610 s
        let stat = "27871 (maker-connect ag) S 1 27871 27871 0 -1 4194560 1 0 0 0 1 1 0 0 20 0 9 0 1000 54052";
        assert_eq!(process_age(stat, "3610.50 12000.00"), Some(3600));
        assert_eq!(duration(3600), "1 h 0 min");
        assert_eq!(duration(59), "0 min");
        assert_eq!(duration(90_000), "1 d 1 h");
    }
}
