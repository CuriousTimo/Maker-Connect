//! A print to take up again after a power cut, on Prusa's MK3 firmware (its power panic,
//! "UVLO"). What the firmware does (`power_panic.cpp`, `Marlin_main.cpp`; read 2026-11-08):
//!
//! - when the power fails during a host's print it saves the line number (`N`) of the last
//!   line it carried out (the newest it took, less what still waited in its buffers);
//! - started again, it waits, paused, and tells a host that is there (`M79`) with
//!   `//action:uvlo_recovery_ready` (`uvlo_auto_recovery_ready` while its bed is still warm);
//! - resumed (`M602`, or its own menu), it heats, moves back and sets its line number to the
//!   saved one itself, then asks the host for the next line: `Resend: N+1`.
//!
//! So the host needs to know which line of the file goes with which `N`. While a job runs
//! the agent keeps a copy of its G-code and a checkpoint beside its credentials: the line
//! number and the place in the file of the next line to go, and the numbers of the last
//! lines that were not the file's (`M79`, an `M105` asking after it): between those the
//! numbers and the file's lines go one for one. Written after each such line and every two
//! seconds; gone when the job ends.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// How many numbers of lines that were not the file's are kept (the printer's saved line
/// is at most its buffers behind the last line sent).
const EXTRAS_KEPT: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub job_id: i64,
    pub name: String,
    /// The line number the next line goes with.
    pub number: u32,
    /// Which of the file's code lines (comments left out) goes next.
    pub index: usize,
    /// How many code lines the file has.
    pub total: usize,
    /// Numbers of recent lines that were not the file's, below `number`.
    pub extras: Vec<u32>,
}

impl Checkpoint {
    /// The code line the printer asks for again (`Resend: line`) after a power cut: counted
    /// back from the checkpoint, the lines that were not the file's left out. None for a
    /// line before what is known.
    pub fn index_of(&self, line: u32) -> Option<usize> {
        // After the checkpoint only the file's lines went (one is written after every other
        // line): one for one, to the file's end at most.
        if line >= self.number {
            return Some((self.index + (line - self.number) as usize).min(self.total));
        }
        let back = self.number - line;
        let extras = self
            .extras
            .iter()
            .filter(|e| **e >= line && **e < self.number)
            .count() as u32;
        let file_lines_back = back.checked_sub(extras)? as usize;
        // The oldest extra kept bounds what can be counted back.
        if self.extras.len() >= EXTRAS_KEPT && self.extras.iter().min().is_some_and(|m| *m > line) {
            return None;
        }
        self.index.checked_sub(file_lines_back)
    }

    /// Keeps the newest numbers of lines that were not the file's.
    pub fn keep_extras(mut extras: Vec<u32>) -> Vec<u32> {
        let excess = extras.len().saturating_sub(EXTRAS_KEPT);
        extras.drain(..excess);
        extras
    }
}

/// Where the checkpoint and the copy of the G-code are kept.
#[derive(Debug, Clone)]
pub struct Recovery {
    base: PathBuf,
}

impl Recovery {
    /// Beside the agent's credentials file.
    pub fn beside(state: &Path) -> Self {
        Self {
            base: state.to_path_buf(),
        }
    }

    fn path(&self, ext: &str) -> PathBuf {
        let mut name = self.base.file_name().unwrap_or_default().to_os_string();
        name.push(ext);
        self.base.with_file_name(name)
    }

    /// A job begins: its G-code is kept until it ends.
    pub fn begin(&self, gcode: &str, checkpoint: &Checkpoint) -> io::Result<()> {
        write_whole(&self.path(".recovery.gcode"), gcode.as_bytes())?;
        self.save(checkpoint)
    }

    pub fn save(&self, checkpoint: &Checkpoint) -> io::Result<()> {
        let json = serde_json::to_vec(checkpoint).map_err(io::Error::other)?;
        write_whole(&self.path(".recovery.json"), &json)
    }

    /// The job to take up, and its G-code, if one was cut off.
    pub fn load(&self) -> Option<(Checkpoint, String)> {
        let checkpoint = serde_json::from_slice(&std::fs::read(self.path(".recovery.json")).ok()?).ok()?;
        let gcode = std::fs::read_to_string(self.path(".recovery.gcode")).ok()?;
        Some((checkpoint, gcode))
    }

    /// The job ended (or will not be taken up): nothing kept.
    pub fn clear(&self) {
        let _ = std::fs::remove_file(self.path(".recovery.json"));
        let _ = std::fs::remove_file(self.path(".recovery.gcode"));
    }
}

/// Written whole or not at all: a power cut while writing leaves the old one.
fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".new");
    let temp = PathBuf::from(temp);
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counted back from the checkpoint, the lines that were not the file's left out.
    #[test]
    fn the_saved_line_is_found_in_the_file() {
        // Sent: N10..=14 file lines 0..=4, N15 an M79, N16..=19 file lines 5..=8; next N20 = line 9.
        let cp = Checkpoint {
            job_id: 1,
            name: "x".into(),
            number: 20,
            index: 9,
            total: 100,
            extras: vec![15],
        };
        // The printer carried out N17 (file line 6): it asks for N18, file line 7.
        assert_eq!(cp.index_of(18), Some(7));
        // It asks for N16: file line 5.
        assert_eq!(cp.index_of(16), Some(5));
        // It asks for N15 (the M79): the next file line, 5.
        assert_eq!(cp.index_of(15), Some(5));
        // Before the M79: N12 is file line 2.
        assert_eq!(cp.index_of(12), Some(2));
        assert_eq!(cp.index_of(20), Some(9));
        // After the checkpoint only file lines went: N25 is file line 14.
        assert_eq!(cp.index_of(25), Some(14));
        assert_eq!(cp.index_of(500), Some(100), "not beyond the file's end");
    }

    #[test]
    fn kept_whole_and_cleared() {
        let dir = std::env::temp_dir().join(format!("mc-recovery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let recovery = Recovery::beside(&dir.join("agent.json"));
        let cp = Checkpoint {
            job_id: 7,
            name: "cube.gcode".into(),
            number: 3,
            index: 0,
            total: 2,
            extras: vec![],
        };
        recovery.begin("G28\nG1 X1\n", &cp).unwrap();
        let (loaded, gcode) = recovery.load().unwrap();
        assert_eq!((loaded, gcode.as_str()), (cp, "G28\nG1 X1\n"));
        recovery.clear();
        assert!(recovery.load().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
