//! The print stats PrusaLink inlines into a serial print: `M73`, for the printer's screen (#187).
//!
//! Ported from PrusaLink `printer_adapter/print_stats.py` (`track_new_print`, :30-57),
//! `file_printer.py` (`print_gcode` :261-286, `send_print_stats` :315-331, `to_print_stats`
//! :333-344), `util.py` (`get_gcode` :133-143, `get_print_stats_gcode` :187-196) and `const.py`
//! (`STATS_EVERY` :122, `TAIL_COMMANDS` :123).
//!
//! Differences (all OURS, the owner's decision of 2026-11-22 in #187):
//!  1. The numbers are not `PrintStats.get_stats` (:74-103) but OctoPrint's estimate (#186, the
//!     number the hub shows too), rounded as `get_stats` rounds: `round()` of the percent and of the
//!     minutes (Python's round half to even). `get_stats` returns 100 percent at
//!     `gcode_number == total_gcode_count - TAIL_COMMANDS` (:100-101); that is part of its numbers
//!     and is not applied: the percent stays OctoPrint's.
//!  2. Only for a printer whose M115 says `Cap:BUILD_PERCENT:1` (`Marlin/src/gcode/host/M115.cpp:185`);
//!     a Prusa always knows `M73`.
//!  3. When OctoPrint has no number yet (`completion` or `printTimeLeft` is `None`) nothing is sent;
//!     PrusaLink always has both.

use super::unidecode::unidecode;
use std::path::Path;

/// `const.py:122`
pub const STATS_EVERY: i64 = 100;
/// `const.py:123`: "how many commands after the last progress report"
pub const TAIL_COMMANDS: i64 = 10;

/// Python `str.strip()` without arguments
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `util.py:133 get_gcode(line)`: the comment cut off, stripped, made ASCII
pub fn get_gcode(line: &str) -> String {
    let before_comment = line.split(';').next().unwrap_or("");
    unidecode(py_strip(before_comment))
}

/// `util.py:187 get_print_stats_gcode(...)`, with the function's own defaults of -1
pub fn get_print_stats_gcode(
    quiet_percent: i64,
    quiet_left: i64,
    quiet_change_in: i64,
    normal_percent: i64,
    normal_left: i64,
    normal_change_in: i64,
) -> String {
    format!(
        "M73 Q{quiet_percent} S{quiet_left} C{quiet_change_in} \
         P{normal_percent} R{normal_left} D{normal_change_in}"
    )
}

/// Python's `for line in text_file`: lines end at `\n`, `\r` or `\r\n` (universal newlines)
fn universal_lines(text: &str) -> Vec<&str> {
    let mut lines = vec![];
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(&text[start..=i]);
                start = i + 1;
            }
            b'\r' => {
                let end = if bytes.get(i + 1) == Some(&b'\n') {
                    i + 1
                } else {
                    i
                };
                lines.push(&text[start..=end]);
                start = end + 1;
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// What `print_stats.py` keeps about the file being printed (`PrintStatsData`), plus the
/// `gcode_number` of `file_printer.py` (`FilePrinterData.gcode_number`)
#[derive(Debug, Clone, PartialEq)]
pub struct PrintStats {
    pub total_gcode_count: i64,
    pub has_inbuilt_stats: bool,
    pub gcode_number: i64,
}

impl PrintStats {
    /// `print_stats.py:30 track_new_print(file_path)` (without `from_gcode_number`) over the
    /// file's text. `Err` when the file is not UTF-8 (Python's `open(.., encoding='utf-8')` raises).
    pub fn track_new_print(file_path: &Path) -> std::io::Result<PrintStats> {
        let bytes = std::fs::read(file_path)?;
        let text = String::from_utf8(bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(PrintStats::of_text(&text))
    }

    /// The loop of `track_new_print`
    pub fn of_text(text: &str) -> PrintStats {
        let mut stats = PrintStats {
            total_gcode_count: 0,
            has_inbuilt_stats: false,
            gcode_number: 0,
        };
        for line in universal_lines(text) {
            let gcode = get_gcode(line);
            if !gcode.is_empty() {
                stats.total_gcode_count += 1;
            }
            if gcode.contains("M73") {
                stats.has_inbuilt_stats = true;
                break;
            }
        }
        stats
    }

    /// `file_printer.py:333 to_print_stats(gcode_number)`
    pub fn to_print_stats(&self, gcode_number: i64) -> bool {
        let divisible = gcode_number % STATS_EVERY == 0;
        let do_stats = !self.has_inbuilt_stats;
        let print_ending = gcode_number == self.total_gcode_count - TAIL_COMMANDS;
        do_stats && (divisible || print_ending)
    }

    /// `file_printer.py:261 print_gcode(gcode)`, the counting and the decision: the file's next
    /// G-code is about to be sent; true when `send_print_stats` comes first
    pub fn next_gcode(&mut self) -> bool {
        self.gcode_number += 1;
        self.to_print_stats(self.gcode_number)
    }
}

/// `file_printer.py:315 send_print_stats()`'s command, from OctoPrint's numbers (difference 1,
/// 3): "let's report both modes the same"
pub fn print_stats_command(
    completion: Option<f64>,
    print_time_left: Option<i64>,
) -> Option<String> {
    let percent_done = completion?.round_ties_even() as i64;
    let min_remaining = (print_time_left? as f64 / 60.0).round_ties_even() as i64;
    Some(get_print_stats_gcode(
        percent_done,
        min_remaining,
        -1,
        percent_done,
        min_remaining,
        -1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::prusalink::python;
    use serde_json::json;

    fn gcodes(n: usize, newline: &str) -> String {
        (0..n)
            .map(|i| match i % 4 {
                0 => format!("G1 X{i} ; a comment{newline}"),
                1 => format!("; only a comment{newline}{newline}"),
                2 => format!("  G1 Y{i}  {newline}"),
                _ => format!("G1 E{i}{newline}"),
            })
            .collect()
    }

    /// PrusaLink's `get_print_stats_gcode` through `send_print_stats`, `track_new_print`,
    /// `to_print_stats` and `print_gcode` on the same files: the same lines in the same order,
    /// `M73` lines before the G-code, and the numbers' rounding
    #[test]
    fn identical_to_prusalink() {
        let dir = std::env::temp_dir().join(format!("maker-connect-m73-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let texts: Vec<(&str, String)> = vec![
            ("none", gcodes(250, "\n")),
            ("crlf", gcodes(250, "\r\n")),
            ("cr", gcodes(230, "\r")),
            (
                "inbuilt",
                format!("G28\nG1 X1\nM73 P0 R10\n{}", gcodes(250, "\n")),
            ),
            (
                "inbuilt_in_comment_only",
                format!("; M73 P5\n{}", gcodes(150, "\n")),
            ),
            (
                "inbuilt_substring",
                format!("G1 X1\nM7300\n{}", gcodes(150, "\n")),
            ),
            (
                "tail_is_100",
                (0..110).map(|i| format!("G1 X{i}\n")).collect(),
            ),
            (
                "tail_is_101",
                (0..111).map(|i| format!("G1 X{i}\n")).collect(),
            ),
            (
                "tail_is_95",
                (0..105).map(|i| format!("G1 X{i}\n")).collect(),
            ),
            (
                "tail_is_90",
                (0..100).map(|i| format!("G1 X{i}\n")).collect(),
            ),
            ("five", (0..5).map(|i| format!("G1 X{i}\n")).collect()),
            ("empty", String::new()),
            (
                "unicode",
                format!(
                    "G1 X1 ; é\nM117 Ünï\né;\n\u{80}\n北亰\n{}",
                    gcodes(210, "\n")
                ),
            ),
            ("no_last_newline", format!("{}G1 X999", gcodes(120, "\n"))),
        ];
        let mut paths = vec![];
        for (name, text) in &texts {
            let path = dir.join(format!("{name}.gcode"));
            std::fs::write(&path, text).unwrap();
            paths.push(path.to_string_lossy().into_owned());
        }
        let completions = [0.0, 0.4, 0.5, 1.5, 2.5, 36.49, 36.5, 49.999, 99.5, 100.0];
        let seconds = [0i64, 1, 29, 30, 31, 89, 90, 91, 150, 210, 3599, 3600, 86399];
        let mut rounding = vec![];
        for c in completions {
            for s in seconds {
                rounding.push((c, s));
            }
        }
        let Some(theirs) = python::run(
            include_str!("driver_m73.py"),
            &json!({"files": paths, "rounding": rounding}),
        ) else {
            return;
        };

        let mut moments = 0;
        for (i, (name, text)) in texts.iter().enumerate() {
            let mut stats = PrintStats::of_text(text);
            let want = &theirs["files"][i];
            assert_eq!(want["total"], stats.total_gcode_count, "{name}");
            assert_eq!(want["inbuilt"], stats.has_inbuilt_stats, "{name}");
            assert_eq!(
                stats,
                PrintStats::track_new_print(Path::new(&paths[i])).unwrap()
            );
            let mut ours: Vec<String> = vec![];
            for line in universal_lines(text) {
                let gcode = get_gcode(line);
                if gcode.is_empty() {
                    continue;
                }
                if stats.next_gcode() {
                    let n = stats.gcode_number;
                    // the numbers of the driver's `get_stats`: percent and minutes
                    let command = print_stats_command(
                        Some(((n * 7) % 101) as f64),
                        Some(((n * 13) % 977) * 60),
                    );
                    ours.push(command.unwrap());
                    moments += 1;
                }
                ours.push(gcode);
            }
            // PrusaLink enqueues every line to the front (the M73 lines and the G-code)
            let theirs_lines: Vec<String> = want["enqueued"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    assert_eq!(e[1], true, "{name}: to_front");
                    e[0].as_str().unwrap().to_string()
                })
                .collect();
            assert_eq!(theirs_lines, ours, "{name}");
        }
        // the cases do have M73 moments (250 lines: 100, 200 and the tail; ...)
        assert!(moments >= 12, "{moments}");

        for (i, (c, s)) in rounding.iter().enumerate() {
            assert_eq!(
                theirs["rounding"][i],
                print_stats_command(Some(*c), Some(*s)).unwrap(),
                "{c} {s}"
            );
        }
        // nothing is sent without a number
        assert_eq!(None, print_stats_command(None, Some(60)));
        assert_eq!(None, print_stats_command(Some(1.0), None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
