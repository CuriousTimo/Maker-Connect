//! What the computer a hub or an agent runs on is, and what it does now: the About pages of
//! both (ours, UI only). Read with `sysinfo` on Linux, macOS and Windows; what a system does
//! not tell is `None` and left off the page.

use sysinfo::{Components, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System};

/// The computer.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Computer {
    /// The distribution or system: `Linux 11 Debian GNU/Linux`, `macOS 15.1 Sequoia`.
    pub system: Option<String>,
    pub kernel: Option<String>,
    /// The board or computer: `Xunlong Orange Pi PC`, `MacBookPro18,3`.
    pub board: Option<String>,
    pub architecture: String,
    pub cpu: Option<String>,
    pub cores: Option<usize>,
    /// Bytes.
    pub memory: Option<u64>,
    pub host: Option<String>,
}

/// What the computer does now.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Load {
    /// Over 1, 5 and 15 minutes (not on Windows).
    pub average: Option<[f64; 3]>,
    /// °C of the processor, or of the first sensor that has one.
    pub temperature: Option<f32>,
    /// Bytes in use and in all.
    pub memory: Option<(u64, u64)>,
    /// Seconds since the computer started.
    pub up: Option<u64>,
}

/// This program.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Process {
    /// Bytes.
    pub memory: Option<u64>,
    /// Seconds since it started.
    pub running: Option<u64>,
}

fn some(text: String) -> Option<String> {
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

pub fn computer() -> Computer {
    let mut sys = System::new().ok();
    if let Some(sys) = sys.as_mut() {
        sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        sys.refresh_cpu_all();
    }
    let cpus = sys.as_ref().map_or(&[][..], |s| s.cpus());
    Computer {
        system: System::long_os_version().ok().and_then(some),
        kernel: System::kernel_long_version().ok().and_then(some),
        board: sysinfo::Product::name().ok().and_then(some),
        architecture: std::env::consts::ARCH.to_owned(),
        cpu: cpus.first().and_then(|c| some(c.brand().to_owned())),
        cores: (!cpus.is_empty()).then_some(cpus.len()),
        memory: sys.as_ref().map(|s| s.total_memory()).filter(|&m| m > 0),
        host: System::host_name().ok().and_then(some),
    }
}

pub fn load() -> Load {
    let mut sys = System::new().ok();
    if let Some(sys) = sys.as_mut() {
        sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    }
    let temperature = Components::new_with_refreshed_list().ok().and_then(|list| {
        let with: Vec<_> = list
            .iter()
            .filter_map(|c| Some((c.label().to_lowercase(), c.temperature()?)))
            .collect();
        with.iter()
            .find(|(label, _)| label.contains("cpu"))
            .or(with.first())
            .map(|(_, t)| *t)
    });
    Load {
        average: System::load_average()
            .ok()
            .map(|l| [l.one, l.five, l.fifteen]),
        temperature,
        memory: sys
            .as_ref()
            .map(|s| (s.used_memory(), s.total_memory()))
            .filter(|&(_, total)| total > 0),
        up: System::uptime().ok(),
    }
}

pub fn process() -> Process {
    let (Ok(pid), Ok(mut sys)) = (sysinfo::get_current_pid(), System::new()) else {
        return Process::default();
    };
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing().with_memory(),
    );
    let me = sys.process(pid);
    Process {
        memory: me.map(|p| p.memory()).filter(|&m| m > 0),
        running: me.map(|p| p.run_time()),
    }
}

/// `1.5 GB`, `512 MB`, `12 kB`.
pub fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.0} MB", n as f64 / (1u64 << 20) as f64),
        n => format!("{:.0} kB", n as f64 / 1024.0),
    }
}

/// `3 d 4 h`, `5 h 12 min`, `40 min`.
pub fn duration(seconds: u64) -> String {
    let (d, h, m) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        _ => format!("{d} d {h} h"),
    }
}

/// The rows of the About pages' tables (the hub's page leads), label and value, leaving out
/// what is not known.
pub fn computer_rows(c: &Computer) -> Vec<(&'static str, String)> {
    let processor = match (&c.cpu, c.cores) {
        (Some(cpu), Some(n)) => Some(format!("{cpu}, {n} cores")),
        (None, Some(n)) => Some(format!("{n} cores")),
        (cpu, None) => cpu.clone(),
    };
    [
        ("System", c.system.clone()),
        ("Kernel", c.kernel.clone()),
        ("Computer", c.board.clone()),
        ("Processor", processor),
        ("Architecture", Some(c.architecture.clone())),
        ("Memory", c.memory.map(bytes)),
        ("Host name", c.host.clone()),
    ]
    .into_iter()
    .filter_map(|(label, value)| Some((label, value?)))
    .collect()
}

pub fn load_rows(l: &Load) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if let Some([a, b, c]) = l.average {
        rows.push(("Load", format!("{a:.2} · {b:.2} · {c:.2} (1, 5, 15 min)")));
    }
    if let Some(t) = l.temperature {
        rows.push(("Temperature", format!("{t:.1} °C")));
    }
    if let Some((used, total)) = l.memory {
        rows.push((
            "Memory in use",
            format!(
                "{} of {} ({}%)",
                bytes(used),
                bytes(total),
                used * 100 / total
            ),
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_values_are_written() {
        assert_eq!(bytes(1015636 * 1024), "992 MB");
        assert_eq!(bytes(3 << 30), "3.0 GB");
        assert_eq!(duration(3600), "1 h 0 min");
        assert_eq!(duration(59), "0 min");
        assert_eq!(duration(90_000), "1 d 1 h");
        let l = Load {
            average: Some([0.5, 0.25, 0.1]),
            temperature: Some(36.07),
            memory: Some((1 << 29, 1 << 30)),
            up: Some(7200),
        };
        assert_eq!(
            load_rows(&l),
            vec![
                ("Load", "0.50 · 0.25 · 0.10 (1, 5, 15 min)".to_owned()),
                ("Temperature", "36.1 °C".to_owned()),
                ("Memory in use", "512 MB of 1.0 GB (50%)".to_owned()),
            ]
        );
    }

    /// On the computer the tests run on: something is known, and nothing is empty.
    #[test]
    fn this_computer() {
        let c = computer();
        assert!(computer_rows(&c).iter().all(|(_, v)| !v.is_empty()));
        assert!(c.cores.is_some() && c.memory.is_some());
        assert!(load().memory.is_some());
        let p = process();
        assert!(p.memory.is_some(), "{p:?}");
        eprintln!("{c:#?}\n{:#?}\n{p:#?}", load());
    }
}
