//! A GRBL machine (a laser) on the agent's serial port, through LaserFoundry's
//! `laserfoundry-device` (MIT OR Apache-2.0): its `Link` keeps the connection on a thread of
//! its own, asks the status every 200 ms, streams a job by GRBL's character counting, and
//! stops by a soft reset, which switches the laser off. Here: what the cloud asks of it, and
//! what it is doing, in Maker Connect's terms. Jogs go as GRBL's own `$J=` jogs (the laser off,
//! cancellable), the settings that matter before a job are read with `$$`, and an interrupted
//! job can start again a little before where it stopped, with LaserFoundry's `restart_line` and
//! `laserfoundry-gcode`'s `resume_from` (which rebuilds where it was with the beam off).

/// LaserFoundry's in-memory GRBL: for tests, and behind the cloud's virtual lasers.
pub use laserfoundry_device::SimulatedGrbl;
use laserfoundry_device::{JobState, Link, MachineState as Grbl, Snapshot, Transport};
use maker_connect_protocol::{
    GrblSettings, JobControl, JobEnd, JobProgress, MachineState, Telemetry,
};
use std::time::{Duration, Instant};

/// How long a command may take to be answered (homing takes a while).
const ANSWER_WITHIN: Duration = Duration::from_secs(90);

pub struct GrblMachine {
    link: Link,
    job: Option<Running>,
    firmware: Option<String>,
}

/// The job this machine runs.
struct Running {
    id: i64,
    name: String,
    /// What was streamed (the file, or the file rebuilt from a line).
    streamed: String,
    /// The file line the stream started at, and how many lines the rebuilt start added.
    from_line: Option<(u64, u64)>,
    started: Instant,
    paused_for: Duration,
    paused_at: Option<Instant>,
    stopping: bool,
}

impl GrblMachine {
    pub fn new(transport: Box<dyn Transport + Send>) -> Self {
        let link = Link::connect(transport);
        // The settings that matter before a job ($30, $32, $130 to $132).
        let _ = link.request_settings();
        GrblMachine {
            link,
            job: None,
            firmware: None,
        }
    }

    /// Opens a GRBL machine on a serial port.
    #[cfg(feature = "serial")]
    pub fn open(port: &str, baud: u32) -> std::io::Result<Self> {
        let transport = laserfoundry_device::serial::SerialTransport::open(port, baud)?;
        Ok(Self::new(Box::new(transport)))
    }

    pub fn running(&self) -> Option<i64> {
        self.job.as_ref().map(|j| j.id)
    }

    /// Carries out a command's G-code (`$H` homes, `$X` unlocks, anything else is sent as
    /// it is), waits for its answers, and returns what the machine said.
    pub fn carry_out(&mut self, gcode: &str) -> Result<Vec<String>, String> {
        if self.job.is_some() {
            return Err("a job is running".to_owned());
        }
        let before = self.link.snapshot().console;
        let mut sent = 0;
        for line in gcode.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let upper = line.to_ascii_uppercase();
            let result = match upper.as_str() {
                "$H" => self.link.home(),
                "$X" => self.link.unlock(),
                "$$" => self.link.request_settings(),
                // A jog through the link, which checks it and keeps the laser off.
                _ if upper.starts_with("$J=") => match jog(&upper) {
                    Some(Jog::By(dx, dy, feed)) => self.link.jog(dx, dy, feed),
                    Some(Jog::To(x, y, feed)) => self.link.jog_to(x, y, feed),
                    None => return Err(format!("not a jog this machine takes: {line}")),
                },
                _ => self.link.send_line(line),
            };
            result.map_err(|e| e.to_string())?;
            sent += 1;
        }
        // Each line is answered `ok` or `error:N`, homing once it is done.
        let deadline = Instant::now() + ANSWER_WITHIN;
        loop {
            let said = new_lines(&before, &self.link.snapshot().console);
            let answers = said.iter().filter(|l| is_answer(l)).count();
            if answers >= sent || Instant::now() > deadline {
                let failed = said
                    .iter()
                    .find(|l| l.contains("error:") || l.contains("ALARM:"));
                return match failed {
                    Some(line) => Err(line.trim_start_matches("< ").to_owned()),
                    None => Ok(said.iter().map(|l| tidy(l)).collect()),
                };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Starts a job: the program streamed by the link; from a 1-based line of it when given
    /// (resuming), rebuilt by `resume_from` so the head first goes there with the beam off.
    pub fn start(
        &mut self,
        id: i64,
        name: &str,
        program: String,
        from_line: Option<u64>,
    ) -> Result<(), String> {
        let (streamed, from_line) = match from_line {
            Some(line) if line > 1 => {
                let rebuilt = laserfoundry_gcode::resume_from(&program, line as usize)
                    .map_err(|e| format!("cannot start at line {line}: {e}"))?;
                // The rebuilt start, before the file's own lines from `line` on.
                let tail = program.lines().count() as u64 + 1 - line;
                let added = (rebuilt.lines().count() as u64).saturating_sub(tail);
                (rebuilt, Some((line, added)))
            }
            _ => (program, None),
        };
        let before = self.link.snapshot().job;
        self.link
            .start_job(streamed.clone())
            .map_err(|e| e.to_string())?;
        // Until the link's thread takes the job up, its snapshot still says how the one
        // before ended: wait for that to change, so it is not taken for this job's end.
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until
            && self.link.snapshot().job == before
            && before != JobState::Idle
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.job = Some(Running {
            id,
            name: name.to_owned(),
            streamed,
            from_line,
            started: Instant::now(),
            paused_for: Duration::ZERO,
            paused_at: None,
            stopping: false,
        });
        Ok(())
    }

    pub fn control(&mut self, control: JobControl) {
        let Some(job) = self.job.as_mut() else { return };
        let done = match control {
            JobControl::Pause if job.paused_at.is_none() => {
                job.paused_at = Some(Instant::now());
                self.link.pause()
            }
            JobControl::Resume => {
                if let Some(at) = job.paused_at.take() {
                    job.paused_for += at.elapsed();
                }
                self.link.resume()
            }
            JobControl::Stop if !job.stopping => {
                job.stopping = true;
                self.link.stop()
            }
            _ => Ok(()),
        };
        if let Err(error) = done {
            println!("job {}: {error}", job.id);
        }
    }

    /// What the machine is doing now; and the job that ended since the last time, if one did.
    pub fn report(&mut self) -> (Telemetry, Option<JobEnd>) {
        let snapshot = self.link.snapshot();
        if self.firmware.is_none() {
            self.firmware = banner(&snapshot);
        }
        let mut ended = None;
        if let (Some(job), JobState::Finished(result)) = (&self.job, &snapshot.job) {
            let ok = result.is_ok() && !job.stopping;
            let acknowledged = snapshot.progress.map_or(0, |p| p.acknowledged);
            ended = Some(JobEnd {
                id: job.id,
                ok,
                stopped: job.stopping,
                reason: result.as_ref().err().cloned(),
                restart_line: (!ok && acknowledged > 0).then(|| job.file_line(acknowledged)),
            });
            self.job = None;
        }
        (self.telemetry(&snapshot), ended)
    }

    fn telemetry(&self, snapshot: &Snapshot) -> Telemetry {
        let status = snapshot.status.as_ref();
        let mut state = match status.map(|s| s.state) {
            _ if !snapshot.connected => MachineState::Disconnected,
            Some(Grbl::Run | Grbl::Jog | Grbl::Home | Grbl::Check) => MachineState::Busy,
            Some(Grbl::Hold | Grbl::Door) => MachineState::Paused,
            Some(Grbl::Alarm) => MachineState::Error,
            _ => MachineState::Idle,
        };
        if snapshot.connected {
            match snapshot.job {
                JobState::Running => state = MachineState::Busy,
                JobState::Paused => state = MachineState::Paused,
                _ => {}
            }
        }
        let job = self.job.as_ref().map(|job| {
            let progress = snapshot.progress;
            let (done, total) =
                progress.map_or((0, 0), |p| (p.acknowledged as u64, p.total as u64));
            let paused = job.paused_for + job.paused_at.map_or(Duration::ZERO, |at| at.elapsed());
            let elapsed = job.started.elapsed().saturating_sub(paused).as_secs();
            let fraction = done as f64 / total.max(1) as f64;
            JobProgress {
                file: job.name.clone(),
                done,
                total,
                elapsed_s: Some(elapsed),
                remaining_s: (fraction > 0.02)
                    .then(|| (elapsed as f64 * (1.0 - fraction) / fraction) as u64),
            }
        });
        Telemetry {
            state,
            grbl: settings(snapshot),
            firmware: self.firmware.clone().or(Some("GRBL".to_owned())),
            position_mm: status
                .and_then(|s| s.position)
                .map(|(x, y, z)| [x as f32, y as f32, z as f32]),
            job_id: self.running(),
            job,
            ..Telemetry::default()
        }
    }
}

impl Running {
    /// The file line to start again from after `acknowledged` lines of the stream: a few
    /// lines back (the controller acknowledges a line when it buffers it, not when it has
    /// run), and in the file's numbering when the stream was rebuilt from a line.
    fn file_line(&self, acknowledged: usize) -> u64 {
        let line = laserfoundry_device::restart_line(&self.streamed, acknowledged) as u64;
        match self.from_line {
            Some((from, added)) => from + line.saturating_sub(added + 1),
            None => line,
        }
    }
}

/// A jog: by so much, or to a place (machine coordinates), at a feed in mm/min.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Jog {
    By(f64, f64, f64),
    To(f64, f64, f64),
}

/// `$J=G91 G21 X10 F3000` (by) or `$J=G90 G21 X20 Y30 F3000` (to); X and Y only.
fn jog(line: &str) -> Option<Jog> {
    let words = line.strip_prefix("$J=")?;
    let mut relative = false;
    let (mut x, mut y, mut feed) = (None, None, None);
    for word in words.split_whitespace() {
        let (letter, value) = word.split_at(1);
        match (letter, value) {
            ("G", "91") => relative = true,
            ("G", "90" | "21" | "53") => {}
            ("X", v) => x = Some(v.parse::<f64>().ok().filter(|v| v.is_finite())?),
            ("Y", v) => y = Some(v.parse::<f64>().ok().filter(|v| v.is_finite())?),
            ("F", v) => feed = Some(v.parse::<f64>().ok().filter(|v| *v > 0.0)?),
            _ => return None,
        }
    }
    let feed = feed?;
    if relative {
        Some(Jog::By(x.unwrap_or(0.0), y.unwrap_or(0.0), feed))
    } else {
        Some(Jog::To(x?, y?, feed))
    }
}

/// The settings that matter before a job, once the controller has reported them.
fn settings(snapshot: &Snapshot) -> Option<GrblSettings> {
    let number = |id: u32| {
        snapshot
            .settings
            .get(&id)
            .and_then(|v| v.trim().parse::<f64>().ok())
    };
    if snapshot.settings.is_empty() {
        return None;
    }
    let travel = match (number(130), number(131), number(132)) {
        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
        _ => None,
    };
    Some(GrblSettings {
        laser_mode: number(32).map(|v| v != 0.0),
        max_power: number(30),
        travel_mm: travel,
    })
}

/// GRBL's greeting, `Grbl 1.1f ['$' for help]`, as its firmware: `Grbl 1.1f`.
fn banner(snapshot: &Snapshot) -> Option<String> {
    snapshot.console.iter().find_map(|line| {
        let line = line.trim_start_matches("< ");
        line.starts_with("Grbl ")
            .then(|| line.split(" [").next().unwrap_or(line).trim().to_owned())
    })
}

/// Whether a line greets as GRBL does after a reset (for telling GRBL from Marlin on a port).
pub fn greets(line: &str) -> bool {
    line.trim_start().starts_with("Grbl ")
}

/// The lines of `after` that came since `before`. The console keeps only its last lines, so
/// `after` starts with the end of `before`: the longest such overlap is what was there.
fn new_lines(before: &[String], after: &[String]) -> Vec<String> {
    let kept = (0..=before.len())
        .map(|dropped| &before[dropped..])
        .find(|rest| after.starts_with(rest))
        .map_or(0, <[String]>::len);
    after[kept..].to_vec()
}

fn is_answer(line: &str) -> bool {
    let line = line.trim_start_matches("< ");
    line == "ok"
        || line.starts_with("ok ")
        || line.starts_with("error:")
        || line.starts_with("ALARM:")
}

/// A console line as the terminal shows it: what was sent stays `> line`, what came back
/// loses the link's `< `.
fn tidy(line: &str) -> String {
    line.strip_prefix("< ").unwrap_or(line).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use laserfoundry_device::SimulatedGrbl;

    fn wait_until(
        machine: &mut GrblMachine,
        done: impl Fn(&Telemetry) -> bool,
    ) -> (Telemetry, Option<JobEnd>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ended = None;
        loop {
            let (telemetry, end) = machine.report();
            ended = end.or(ended);
            if done(&telemetry) || Instant::now() > deadline {
                return (telemetry, ended);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A command is answered and its answer comes back; a job streams to the end and says so.
    #[test]
    fn a_grbl_machine_takes_commands_and_jobs() {
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(1));
        let mut machine = GrblMachine::new(Box::new(sim));
        let said = machine.carry_out("$H\nG0 X10 Y5").unwrap();
        assert!(said.iter().filter(|l| *l == "ok").count() >= 2, "{said:?}");
        let (telemetry, _) = wait_until(&mut machine, |t| t.position_mm == Some([10.0, 5.0, 0.0]));
        assert_eq!(telemetry.position_mm, Some([10.0, 5.0, 0.0]));
        assert_eq!(telemetry.state, MachineState::Idle);

        let program = (0..40)
            .map(|i| format!("G1 X{} Y{} S500 F3000", i % 20, i / 2))
            .collect::<Vec<_>>()
            .join("\n");
        machine
            .start(7, "square.gcode", format!("G90\nM4\n{program}\nM5\n"), None)
            .unwrap();
        assert_eq!(machine.report().0.job_id, Some(7));
        assert!(
            machine.carry_out("G0 X0").is_err(),
            "no commands during a job"
        );
        let (_, ended) = wait_until(&mut machine, |t| t.job_id.is_none());
        let ended = ended.unwrap();
        assert!(ended.ok, "{ended:?}");
        assert_eq!(ended.id, 7);
    }

    /// The example files for lasers stream to the end: no line too long, no byte GRBL would
    /// take as a realtime command.
    #[test]
    fn the_laser_examples_stream() {
        for (id, program) in [
            (1, include_str!("../../../examples/laser/first-light.gcode")),
            (
                2,
                include_str!("../../../examples/laser/power-speed-grid.gcode"),
            ),
        ] {
            let mut sim = SimulatedGrbl::new();
            sim.set_latency(Duration::ZERO);
            let mut machine = GrblMachine::new(Box::new(sim));
            machine
                .start(id, "example", program.to_owned(), None)
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(60);
            let ended = loop {
                if let (_, Some(end)) = machine.report() {
                    break end;
                }
                assert!(Instant::now() < deadline, "example {id} did not finish");
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(ended.ok, "example {id}: {ended:?}");
        }
    }

    /// Stopping a job ends it as stopped, not failed.
    #[test]
    fn a_stopped_job_says_so() {
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(5));
        let mut machine = GrblMachine::new(Box::new(sim));
        let program = (0..2000)
            .map(|i| format!("G1 X{} Y1 F3000", i % 50))
            .collect::<Vec<_>>()
            .join("\n");
        machine.start(8, "long.gcode", program, None).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        machine.control(JobControl::Stop);
        let (_, ended) = wait_until(&mut machine, |t| t.job_id.is_none());
        let ended = ended.unwrap();
        assert!(ended.stopped && !ended.ok, "{ended:?}");
    }

    /// The settings come in once connected; jogs go through the link.
    #[test]
    fn settings_are_read_and_jogs_go_through() {
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(1));
        let mut machine = GrblMachine::new(Box::new(sim));
        let (telemetry, _) = wait_until(&mut machine, |t| t.grbl.is_some());
        let settings = telemetry.grbl.unwrap();
        assert_eq!(settings.laser_mode, Some(true));
        assert_eq!(settings.max_power, Some(1000.0));
        assert_eq!(settings.travel_mm, Some([200.0, 200.0, 200.0]));
        machine
            .carry_out("$J=G90 G21 X20 Y30 F3000\n$J=G91 G21 X5 F3000")
            .unwrap();
        let (telemetry, _) = wait_until(&mut machine, |t| t.position_mm == Some([25.0, 30.0, 0.0]));
        assert_eq!(telemetry.position_mm, Some([25.0, 30.0, 0.0]));
        assert!(machine.carry_out("$J=G91 Z5 F100").is_err(), "X and Y only");
    }

    #[test]
    fn jogs_are_read() {
        assert_eq!(
            jog("$J=G91 G21 X10 F3000"),
            Some(Jog::By(10.0, 0.0, 3000.0))
        );
        assert_eq!(jog("$J=G90 G21 X1 Y2 F600"), Some(Jog::To(1.0, 2.0, 600.0)));
        assert_eq!(jog("$J=G90 X1 F600"), None, "to a place needs X and Y");
        assert_eq!(jog("$J=G91 X1"), None, "a feed is needed");
        assert_eq!(jog("$J=G91 XNaN F600"), None);
    }

    /// A stopped job says where to start again, in the file's lines; started there, the
    /// stream goes on from that line.
    #[test]
    fn an_interrupted_job_starts_again() {
        let program: String = std::iter::once("G21\nG90\nM4 S0".to_owned())
            .chain((0..400).map(|i| format!("G1 X{} Y{} S300 F3000", i % 50, i / 50)))
            .chain(std::iter::once("M5".to_owned()))
            .collect::<Vec<_>>()
            .join("\n");
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(2));
        let mut machine = GrblMachine::new(Box::new(sim));
        machine
            .start(9, "rows.gcode", program.clone(), None)
            .unwrap();
        std::thread::sleep(Duration::from_millis(250));
        machine.control(JobControl::Stop);
        let (_, ended) = wait_until(&mut machine, |t| t.job_id.is_none());
        let ended = ended.unwrap();
        let line = ended.restart_line.expect("a line to start again from");
        assert!(line > 1 && line < 404, "{line}");

        machine
            .start(10, "rows.gcode", program.clone(), Some(line))
            .unwrap();
        let (_, ended) = wait_until(&mut machine, |t| t.job_id.is_none());
        let ended = ended.unwrap();
        assert!(ended.ok, "{ended:?}");
        // Stopped again at once, the restart line is in the file's numbering, past where it began.
        let mut sim = SimulatedGrbl::new();
        sim.set_latency(Duration::from_millis(2));
        let mut again = GrblMachine::new(Box::new(sim));
        again.start(11, "rows.gcode", program, Some(line)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        again.control(JobControl::Stop);
        let (_, ended) = wait_until(&mut again, |t| t.job_id.is_none());
        if let Some(next) = ended.unwrap().restart_line {
            assert!(next >= line, "{next} < {line}");
        }
    }

    #[test]
    fn grbl_greets_by_name() {
        assert!(greets("Grbl 1.1f ['$' for help]"));
        assert!(!greets("start"));
        let before = vec!["< ok".to_owned()];
        let after = vec!["< ok".to_owned(), "> G0 X1".to_owned(), "< ok".to_owned()];
        assert_eq!(new_lines(&before, &after), vec!["> G0 X1", "< ok"]);
        let trimmed = vec!["> G0 X1".to_owned(), "< ok".to_owned(), "> M5".to_owned()];
        assert_eq!(
            new_lines(&after, &trimmed),
            vec!["> M5"],
            "the console dropped its first line"
        );
    }
}
