//! Streaming G-code to a printer that speaks the Marlin serial protocol (Marlin, Prusa's
//! firmware, and most other FDM printers on USB), the way OctoPrint does it
//! (`reference/OctoPrint`, `plugins/serial_connector/serial_comm.py`):
//!
//! - every line goes out with a line number and a checksum: `N12 G1 X10*85`, the checksum
//!   being the XOR of every byte before the `*`;
//! - one line at a time: the next one goes once the printer answered `ok`;
//! - `Resend: 12` (or `rs N12`) asks for line 12 and those after it again: the last lines
//!   sent are kept for that;
//! - `busy: processing` and `echo:busy` while it heats or homes: keep waiting;
//! - opening the port resets most printers: wait for `start` (or a quiet moment), then
//!   `M110 N0` sets the line numbers back to zero.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::time::{Duration, Instant};

/// How many sent lines are kept for a resend.
const KEPT_LINES: usize = 64;
/// How long a printer may stay silent before the sender gives up, unless it said busy.
const SILENT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for a printer to finish booting after the port was opened.
const BOOT_WAIT: Duration = Duration::from_secs(10);

/// A G-code line as it goes on the wire, without its comment: `None` for a line that is
/// only a comment or blank.
pub fn clean(line: &str) -> Option<&str> {
    let code = line.split(';').next().unwrap_or("").trim();
    (!code.is_empty()).then_some(code)
}

/// The XOR of every byte: Marlin's line checksum.
pub fn checksum(text: &str) -> u8 {
    text.bytes().fold(0, |sum, byte| sum ^ byte)
}

/// A line numbered and with its checksum, ready to send (without the newline).
pub fn numbered(number: u32, code: &str) -> String {
    let line = format!("N{number} {code}");
    format!("{line}*{}", checksum(&line))
}

/// What a line from the printer means for the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// `ok`, possibly with a temperature report after it.
    Ok,
    /// `Resend: N` or `rs N`: send line N and those after it again.
    Resend(u32),
    /// `busy: processing`, `echo:busy`: it is working; keep waiting.
    Busy,
    /// `start`: it has (re)started.
    Start,
    /// `Error:...`.
    Error(String),
    /// Anything else: temperatures, `echo:` messages.
    Other(String),
}

pub fn parse(line: &str) -> Reply {
    let line = line.trim();
    let lower = line.to_ascii_lowercase();
    if lower == "ok" || lower.starts_with("ok ") {
        return Reply::Ok;
    }
    // Marlin: "Resend: 12"; some firmwares: "rs N12" or "rs 12".
    let resend = lower
        .strip_prefix("resend:")
        .or_else(|| lower.strip_prefix("rs "))
        .map(|rest| rest.trim().trim_start_matches('n').trim());
    if let Some(Ok(number)) = resend.map(str::parse) {
        return Reply::Resend(number);
    }
    if lower.starts_with("busy:") || lower.starts_with("echo:busy") {
        return Reply::Busy;
    }
    if lower == "start" {
        return Reply::Start;
    }
    if lower.starts_with("error:") {
        return Reply::Error(line[6..].trim().to_owned());
    }
    Reply::Other(line.to_owned())
}

/// The firmware from an `M115` answer: `FIRMWARE_NAME:Prusa-Firmware 3.14.0 based on
/// Marlin ...` gives "Prusa-Firmware 3.14.0", `FIRMWARE_NAME:Marlin 2.1.2 (...) SOURCE_...`
/// "Marlin 2.1.2".
pub fn firmware(lines: &[String]) -> Option<String> {
    let line = lines.iter().find_map(|l| l.split_once("FIRMWARE_NAME:"))?.1;
    let end = [" based on", " SOURCE_CODE_URL", " PROTOCOL_VERSION", " ("]
        .iter()
        .filter_map(|marker| line.find(marker))
        .min()
        .unwrap_or(line.len());
    Some(line[..end].trim().to_owned()).filter(|f| !f.is_empty())
}

/// The machine type from an `M115` answer: `MACHINE_TYPE:Prusa-MINI EXTRUDER_COUNT:1`
/// gives "Prusa-MINI", `MACHINE_TYPE:Prusa i3 MK3S EXTRUDER_COUNT:1` "Prusa i3 MK3S".
pub fn machine_type(lines: &[String]) -> Option<String> {
    let line = lines.iter().find_map(|l| l.split_once("MACHINE_TYPE:"))?.1;
    let end = line.find(" EXTRUDER_COUNT").unwrap_or(line.len());
    Some(line[..end].trim().to_owned()).filter(|t| !t.is_empty())
}

/// A Prusa's model code from an `M862.2 Q` answer (`echo:  M862.2 P302`): 302 for an
/// MK3S, 120 for a MINI. Other firmware does not answer it.
pub fn model_code(lines: &[String]) -> Option<u32> {
    let line = lines.iter().find_map(|l| l.split_once("M862.2 P"))?.1;
    line.split_whitespace().next()?.parse().ok()
}

/// The heaters from an `M105` answer, or a temperature report (`M155`, the `ok`s while
/// heating): `ok T:210.3 /210.0 B:60.1 /60.0 T0:210.3 /210.0 @:64 B@:0`. After the `/` is
/// what it heats to (0: off); `@:` and `B@:` are the heaters' power (0: no power now).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Heat {
    pub nozzle_c: Option<f32>,
    pub nozzle_target_c: Option<f32>,
    pub bed_c: Option<f32>,
    pub bed_target_c: Option<f32>,
    pub nozzle_heating: Option<bool>,
    pub bed_heating: Option<bool>,
}

pub fn heat(lines: &[String]) -> Heat {
    // `T:210.3 /210.0`, or written together: `T:210.3/210.0`.
    let reading = |key: &str| {
        lines.iter().find_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            words.iter().enumerate().find_map(|(i, word)| {
                let rest = word.strip_prefix(key)?;
                let (now, target) = match rest.split_once('/') {
                    Some((now, target)) => (now, Some(target)),
                    None => (rest, words.get(i + 1).and_then(|w| w.strip_prefix('/'))),
                };
                let now: f32 = now.parse().ok()?;
                Some((now, target.and_then(|t| t.parse::<f32>().ok())))
            })
        })
    };
    let power = |key: &str| {
        lines.iter().find_map(|line| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix(key))
                .and_then(|v| v.parse::<f32>().ok())
                .map(|p| p > 0.0)
        })
    };
    let nozzle = reading("T:");
    let bed = reading("B:");
    Heat {
        nozzle_c: nozzle.map(|n| n.0),
        nozzle_target_c: nozzle.and_then(|n| n.1),
        bed_c: bed.map(|b| b.0),
        bed_target_c: bed.and_then(|b| b.1),
        nozzle_heating: power("@:"),
        bed_heating: power("B@:"),
    }
}

/// What happened while sending, for the caller to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The printer said something worth showing (an error, a message, temperatures).
    Said(String),
    /// A line was acknowledged: how many of the file's code lines are done.
    Progress { done: usize, total: usize },
    /// The printer asked for a line again.
    Resent(u32),
}

/// A bed mesh in what a printer said, after a header that announces one: Prusa's `G81`
/// ("Measured points:", the back row first, no row numbers), Marlin's bilinear and mesh
/// levelling (`M420 V`: "Bilinear Leveling Grid:", "Mesh Bed Level data:", a header row of
/// column numbers, then each row after its number) and UBL ("Bed Topography Report", rows
/// numbered, `|` between, `.` where a point was not measured). Rows with numbers are put in
/// order by them (row 0 is the front); rows without, as Prusa prints them, are turned round.
pub fn mesh(lines: &[String]) -> Option<maker_connect_protocol::Mesh> {
    const HEADERS: [&str; 4] = [
        "Measured points:",
        "Bilinear Leveling Grid:",
        "Mesh Bed Level data:",
        "Bed Topography Report",
    ];
    let start = lines
        .iter()
        .rposition(|l| HEADERS.iter().any(|h| l.contains(h)))?;
    let mut numbered: Vec<(Option<usize>, Vec<Option<f32>>)> = Vec::new();
    for line in &lines[start + 1..] {
        let cleaned = line.replace(['|', '[', ']', ','], " ");
        let words: Vec<&str> = cleaned.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let value = |w: &str| -> Option<Option<f32>> {
            match w {
                "." | "nan" | "NAN" => Some(None),
                _ if w.contains('.') => w.trim_start_matches('+').parse::<f32>().ok().map(Some),
                _ => None,
            }
        };
        // A row number first (no decimal point), then the values.
        let (index, rest) = match words[0].parse::<usize>() {
            Ok(n) if words.len() > 1 && value(words[1]).is_some() => (Some(n), &words[1..]),
            _ => (None, &words[..]),
        };
        let values: Option<Vec<Option<f32>>> = rest.iter().map(|w| value(w)).collect();
        match values {
            Some(values) if values.len() >= 2 => numbered.push((index, values)),
            // A header row of column numbers, or "(0,4)" corners: not data.
            _ if numbered.is_empty() => continue,
            // The first line after the rows ends them.
            _ => break,
        }
    }
    if numbered.len() < 2 {
        return None;
    }
    let width = numbered[0].1.len();
    if numbered.iter().any(|(_, row)| row.len() != width) {
        return None;
    }
    let rows = if numbered.iter().all(|(i, _)| i.is_some()) {
        numbered.sort_by_key(|(i, _)| *i);
        numbered.into_iter().map(|(_, row)| row).collect()
    } else {
        numbered.into_iter().rev().map(|(_, row)| row).collect()
    };
    Some(maker_connect_protocol::Mesh { rows })
}

/// A connection to a printer: anything that reads and writes bytes, with a read timeout
/// (a serial port; a fake printer in the tests).
pub struct Printer<L: Read + Write> {
    reader: BufReader<L>,
    next_number: u32,
    /// The last lines sent, by number, for a resend.
    sent: VecDeque<(u32, String)>,
}

impl<L: Read + Write> Printer<L> {
    pub fn new(link: L) -> Self {
        Self {
            reader: BufReader::new(link),
            next_number: 1,
            sent: VecDeque::new(),
        }
    }

    /// One line from the printer, or None when it stayed silent for the read timeout.
    fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the printer closed the connection",
            )),
            Ok(_) => Ok(Some(line.trim().to_owned())),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn write_line(&mut self, text: &str) -> io::Result<()> {
        let link = self.reader.get_mut();
        link.write_all(text.as_bytes())?;
        link.write_all(b"\n")?;
        link.flush()
    }

    /// After opening the port: waits for the printer to boot (`start`, then quiet), and
    /// sets its line numbers back to zero (`M110 N0`).
    pub fn connect(&mut self, on: &mut impl FnMut(Event)) -> io::Result<()> {
        let deadline = Instant::now() + BOOT_WAIT;
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline {
            match self.read_line()? {
                Some(line) if !line.is_empty() => {
                    quiet_since = Instant::now();
                    if !matches!(parse(&line), Reply::Ok) {
                        on(Event::Said(line));
                    }
                }
                // A printer that does not reset, or has finished booting, goes quiet.
                _ if quiet_since.elapsed() > Duration::from_secs(2) => break,
                _ => {}
            }
        }
        self.next_number = 0;
        self.sent.clear();
        self.send_numbered("M110 N0")?;
        self.wait_ok(on).map(|_| ())
    }

    /// Sends one command and returns what the printer answered: its lines before the
    /// `ok`, and the `ok` line itself.
    pub fn command(&mut self, code: &str) -> io::Result<Vec<String>> {
        self.send_numbered(code)?;
        let mut lines = Vec::new();
        let ok = self.wait_ok(&mut |event| {
            if let Event::Said(text) = event {
                lines.push(text);
            }
        })?;
        lines.push(ok);
        Ok(lines)
    }

    fn send_numbered(&mut self, code: &str) -> io::Result<()> {
        let number = self.next_number;
        let line = numbered(number, code);
        self.write_line(&line)?;
        self.sent.push_back((number, code.to_owned()));
        if self.sent.len() > KEPT_LINES {
            self.sent.pop_front();
        }
        self.next_number += 1;
        Ok(())
    }

    /// Waits for the `ok` of the last line sent. One line is in flight at a time, so a
    /// `Resend: N` is for that line: Marlin answers a damaged line with `Error:`, then
    /// `Resend: N`, then an `ok` that belongs to the request; at that `ok` the line goes
    /// again, and its own `ok` follows.
    /// Returns the `ok` line, which carries the temperatures after an `M105`.
    fn wait_ok(&mut self, on: &mut impl FnMut(Event)) -> io::Result<String> {
        let mut last_heard = Instant::now();
        let mut resend = None;
        loop {
            let Some(line) = self.read_line()? else {
                if last_heard.elapsed() > SILENT_TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the printer stopped answering",
                    ));
                }
                continue;
            };
            last_heard = Instant::now();
            match parse(&line) {
                Reply::Ok => match resend.take() {
                    None => return Ok(line),
                    Some(number) => {
                        let Some((_, code)) = self.sent.iter().find(|(n, _)| *n == number).cloned()
                        else {
                            return Err(io::Error::other(format!(
                                "the printer asks for line {number}, which is no longer kept"
                            )));
                        };
                        on(Event::Resent(number));
                        self.write_line(&numbered(number, &code))?;
                    }
                },
                Reply::Resend(number) => resend = Some(number),
                Reply::Busy => {}
                Reply::Start => return Err(io::Error::other("the printer restarted")),
                Reply::Error(text) => on(Event::Said(format!("Error: {text}"))),
                Reply::Other(text) if !text.is_empty() => on(Event::Said(text)),
                Reply::Other(_) => {}
            }
        }
    }

    /// Sends one line of a job and waits for its `ok`, which it returns (it carries the
    /// temperatures on most printers).
    pub fn send_line(&mut self, code: &str, on: &mut impl FnMut(Event)) -> io::Result<String> {
        self.send_numbered(code)?;
        self.wait_ok(on)
    }

    /// Sends G-code, line by line, each acknowledged before the next.
    pub fn send<'a>(
        &mut self,
        lines: impl IntoIterator<Item = &'a str>,
        on: &mut impl FnMut(Event),
    ) -> io::Result<()> {
        let codes: Vec<&str> = lines.into_iter().filter_map(clean).collect();
        let total = codes.len();
        for (done, code) in codes.into_iter().enumerate() {
            self.send_numbered(code)?;
            self.wait_ok(on)?;
            on(Event::Progress {
                done: done + 1,
                total,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn said(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    /// Prusa's G81 prints the back row first; the mesh starts at the front.
    #[test]
    fn a_prusa_mesh_is_read_front_first() {
        let lines = said(
            "Num X,Y: 7,7\nZ search height: 5.00\nMeasured points:\n  0.10000  0.11000  0.12000\n  0.05000  0.06000  0.07000\n -0.01000  0.00000  0.01000\nok",
        );
        let mesh = mesh(&lines).unwrap();
        assert_eq!(mesh.rows[0], vec![Some(-0.01), Some(0.0), Some(0.01)]);
        assert_eq!(mesh.rows[2][2], Some(0.12));
        assert_eq!(mesh.range(), Some((-0.01, 0.12)));
    }

    /// Marlin numbers its rows: row 0 is the front, however they are printed.
    #[test]
    fn a_marlin_mesh_is_read_by_row_number() {
        let lines = said(
            "Bilinear Leveling Grid:\n      0      1      2\n 0 +0.012 -0.050 +0.100\n 1 +0.020 +0.000 -0.030\n 2 -0.100 +0.040 +0.060\nok",
        );
        let mesh = mesh(&lines).unwrap();
        assert_eq!(mesh.rows.len(), 3);
        assert_eq!(mesh.rows[0][1], Some(-0.05));
        let ubl = said(
            "Bed Topography Report:\n\n    (0,2)          (2,2)\n        0       1       2\n 2 | +0.100  +0.110   .\n   |\n 1 | +0.050  +0.060  +0.070\n   |\n 0 | -0.010 [+0.000] +0.010\n        0       1       2\n    (0,0)          (2,0)\nok",
        );
        let mesh = super::mesh(&ubl).unwrap();
        assert_eq!(mesh.rows[0], vec![Some(-0.01), Some(0.0), Some(0.01)]);
        assert_eq!(mesh.rows[2][2], None, "not measured");
    }

    #[test]
    fn no_mesh_without_one() {
        assert_eq!(mesh(&said("Mesh bed leveling not active.\nok")), None);
        assert_eq!(mesh(&said("T:21.0 /0.0 B:20.1 /0.0\nok")), None);
    }
    use std::sync::{Arc, Mutex};

    #[test]
    fn lines_get_a_number_and_a_checksum() {
        // As OctoPrint and Marlin compute it: the XOR of "N0 M110 N0".
        assert_eq!(
            numbered(0, "M110 N0"),
            format!("N0 M110 N0*{}", checksum("N0 M110 N0"))
        );
        assert_eq!(checksum("N1 G28"), b"N1 G28".iter().fold(0, |s, b| s ^ b));
        assert_eq!(clean("G1 X10 ; move"), Some("G1 X10"));
        assert_eq!(clean("; only a comment"), None);
        assert_eq!(clean("   "), None);
    }

    #[test]
    fn replies_are_read() {
        assert_eq!(parse("ok"), Reply::Ok);
        assert_eq!(parse("ok T:210.0 /210.0 B:60.0 /60.0"), Reply::Ok);
        assert_eq!(parse("Resend: 12"), Reply::Resend(12));
        assert_eq!(parse("rs N7"), Reply::Resend(7));
        assert_eq!(parse("echo:busy: processing"), Reply::Busy);
        assert_eq!(parse("busy: processing"), Reply::Busy);
        assert_eq!(parse("start"), Reply::Start);
        assert_eq!(
            parse("Error:Line Number is not Last Line Number+1, Last Line: 3"),
            Reply::Error("Line Number is not Last Line Number+1, Last Line: 3".into())
        );
    }

    /// A printer in memory that checks what it gets as Marlin does: the line number and the
    /// checksum, asking for a resend when one is wrong; it can be told to spoil a line
    /// (as noise on the wire would) and to report busy first.
    pub(crate) struct FakeMarlin {
        expected: u32,
        out: VecDeque<u8>,
        input: Vec<u8>,
        /// Line numbers to treat once as arrived damaged.
        spoil: Vec<u32>,
        busy_first: usize,
        /// The codes it accepted, in order.
        accepted: Arc<Mutex<Vec<String>>>,
    }

    impl FakeMarlin {
        pub(crate) fn new(accepted: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                expected: 0,
                out: b"start\necho: Marlin\n".iter().copied().collect(),
                input: Vec::new(),
                spoil: Vec::new(),
                busy_first: 0,
                accepted,
            }
        }

        fn say(&mut self, text: &str) {
            self.out.extend(text.bytes());
            self.out.push_back(b'\n');
        }

        fn receive(&mut self, line: &str) {
            let Some((body, sum)) = line.rsplit_once('*') else {
                return self.say("ok");
            };
            let number: u32 = body[1..].split(' ').next().unwrap().parse().unwrap();
            let code = body.split_once(' ').unwrap().1.to_owned();
            if code.starts_with("M110") {
                self.expected = number + 1;
                return self.say("ok");
            }
            let damaged = self.spoil.contains(&number) || sum.parse::<u8>() != Ok(checksum(body));
            if damaged || number != self.expected {
                self.spoil.retain(|n| *n != number);
                self.say("Error:checksum mismatch");
                self.say(&format!("Resend: {}", self.expected));
                return self.say("ok");
            }
            for _ in 0..std::mem::take(&mut self.busy_first) {
                self.say("echo:busy: processing");
            }
            self.expected += 1;
            match code.as_str() {
                "M115" => self.say("FIRMWARE_NAME:Prusa-Firmware 3.14.0 based on Marlin FIRMWARE_URL:https://github.com/prusa3d/Prusa-Firmware PROTOCOL_VERSION:1.0 MACHINE_TYPE:Prusa i3 MK3S EXTRUDER_COUNT:1"),
                "M105" => return self.say("ok T:24.5 /0.0 B:23.1 /0.0 T0:24.5 /0.0 @:0 B@:0 P:0.0 A:28.2"),
                "M862.2 Q" => self.say("echo:  M862.2 P302"),
                _ => {}
            }
            self.accepted.lock().unwrap().push(code);
            self.say("ok T:210.0 /210.0 B:60.0 /60.0");
        }
    }

    impl Read for FakeMarlin {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.out.is_empty() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "quiet"));
            }
            let n = buf.len().min(self.out.len());
            for byte in buf.iter_mut().take(n) {
                *byte = self.out.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for FakeMarlin {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.input.extend_from_slice(buf);
            while let Some(end) = self.input.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.input.drain(..=end).collect();
                self.receive(String::from_utf8_lossy(&line).trim());
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    const GCODE: &str =
        "; a small print\nG28 ; home\nM104 S210\nG1 X10 Y10\n\nG1 X20 Y10 E1.5\nM84\n";

    fn run(fake: FakeMarlin) -> (Vec<String>, Vec<Event>) {
        let accepted = fake.accepted.clone();
        let mut printer = Printer::new(fake);
        let mut events = Vec::new();
        printer.connect(&mut |e| events.push(e)).unwrap();
        printer
            .send(GCODE.lines(), &mut |e| events.push(e))
            .unwrap();
        let got = accepted.lock().unwrap().clone();
        (got, events)
    }

    /// Every code line arrives once, in order, without comments or blank lines.
    #[test]
    fn a_file_is_streamed_line_by_line() {
        let (accepted, events) = run(FakeMarlin::new(Arc::default()));
        assert_eq!(
            accepted,
            ["G28", "M104 S210", "G1 X10 Y10", "G1 X20 Y10 E1.5", "M84"]
        );
        assert_eq!(events.last(), Some(&Event::Progress { done: 5, total: 5 }));
        assert!(events.contains(&Event::Said("echo: Marlin".into())));
    }

    /// A line that arrives damaged is sent again when the printer asks, and nothing is lost
    /// or doubled.
    #[test]
    fn a_damaged_line_is_resent() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.spoil = vec![3];
        let (accepted, events) = run(fake);
        assert_eq!(
            accepted,
            ["G28", "M104 S210", "G1 X10 Y10", "G1 X20 Y10 E1.5", "M84"]
        );
        assert!(events.contains(&Event::Resent(3)), "{events:?}");
    }

    /// One command and its answer: the firmware from M115, the temperatures from M105.
    #[test]
    fn firmware_and_temperatures_are_read() {
        let mut printer = Printer::new(FakeMarlin::new(Arc::default()));
        printer.connect(&mut |_| {}).unwrap();
        let answer = printer.command("M115").unwrap();
        assert_eq!(firmware(&answer).as_deref(), Some("Prusa-Firmware 3.14.0"));
        let answer = printer.command("M105").unwrap();
        assert_eq!(
            heat(&answer),
            Heat {
                nozzle_c: Some(24.5),
                nozzle_target_c: Some(0.0),
                bed_c: Some(23.1),
                bed_target_c: Some(0.0),
                nozzle_heating: Some(false),
                bed_heating: Some(false),
            }
        );
        // Heating, written together, and Marlin without a power for the bed.
        let heating = heat(&["T:180.2/215.0 B:58.0 /60.0 @:127".to_owned()]);
        assert_eq!(heating.nozzle_target_c, Some(215.0));
        assert_eq!(heating.bed_target_c, Some(60.0));
        assert_eq!(heating.nozzle_heating, Some(true));
        assert_eq!(heating.bed_heating, None);
        let marlin = vec!["FIRMWARE_NAME:Marlin 2.1.2.1 (Jun 1 2024) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0".to_owned()];
        assert_eq!(firmware(&marlin).as_deref(), Some("Marlin 2.1.2.1"));
        let answer = printer.command("M115").unwrap();
        assert_eq!(machine_type(&answer).as_deref(), Some("Prusa i3 MK3S"));
        let answer = printer.command("M862.2 Q").unwrap();
        assert_eq!(model_code(&answer), Some(302));
        let mini = vec!["FIRMWARE_NAME:Prusa-Firmware-Buddy 6.3.0 (Github) SOURCE_CODE_URL:https://github.com/prusa3d/Prusa-Firmware-Buddy PROTOCOL_VERSION:1.0 MACHINE_TYPE:Prusa-MINI EXTRUDER_COUNT:1".to_owned()];
        assert_eq!(
            firmware(&mini).as_deref(),
            Some("Prusa-Firmware-Buddy 6.3.0")
        );
        assert_eq!(machine_type(&mini).as_deref(), Some("Prusa-MINI"));
    }

    /// While the printer reports busy (heating, homing) the sender waits.
    #[test]
    fn a_busy_printer_is_waited_for() {
        let mut fake = FakeMarlin::new(Arc::default());
        fake.busy_first = 3;
        let (accepted, _) = run(fake);
        assert_eq!(accepted.len(), 5);
    }
}
