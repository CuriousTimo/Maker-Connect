//! A print on a Marlin printer over USB: the file streamed line by line on a thread of its
//! own (each line numbered, checksummed, acknowledged; resends as the printer asks), while
//! the agent keeps reporting. It can be paused (between two lines), resumed and stopped
//! (heaters and motors off). The printer reports its temperatures by itself meanwhile
//! (`M155 S2`); the `ok`s carry them too. When the job ends, the printer is handed back.

use crate::marlin::{self, Event, Printer};
use maker_connect_protocol::JobEnd;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the thread and the agent share.
#[derive(Debug, Default)]
struct Shared {
    done: usize,
    total: usize,
    paused: bool,
    stop: bool,
    heat: marlin::Heat,
    /// What the printer said (errors, messages), for the agent's log.
    said: Vec<String>,
}

/// A print running.
pub struct PrintJob<L: Read + Write + Send + 'static> {
    pub id: i64,
    pub name: String,
    shared: Arc<Mutex<Shared>>,
    thread: Option<JoinHandle<(Printer<L>, JobEnd)>>,
    started: std::time::Instant,
}

/// What a job reports now.
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub paused: bool,
    pub heat: marlin::Heat,
    pub elapsed_s: u64,
    pub said: Vec<String>,
}

/// Lines sent when a print is stopped: heaters off, fan off, motors off.
const STOP: [&str; 4] = ["M104 S0", "M140 S0", "M107", "M84"];

impl<L: Read + Write + Send + 'static> PrintJob<L> {
    /// Starts streaming `gcode` to the printer, which the job holds until it ends.
    pub fn start(id: i64, name: &str, gcode: String, mut printer: Printer<L>) -> Self {
        let codes: Vec<String> = gcode
            .lines()
            .filter_map(marlin::clean)
            .map(str::to_owned)
            .collect();
        let shared = Arc::new(Mutex::new(Shared {
            total: codes.len(),
            ..Shared::default()
        }));
        let state = shared.clone();
        let thread = std::thread::spawn(move || {
            let heard = |state: &Arc<Mutex<Shared>>, line: &str| {
                let heard = marlin::heat(&[line.to_owned()]);
                let mut s = state.lock().unwrap();
                let h = &mut s.heat;
                // What this line told; the rest as last heard.
                h.nozzle_c = heard.nozzle_c.or(h.nozzle_c);
                h.nozzle_target_c = heard.nozzle_target_c.or(h.nozzle_target_c);
                h.bed_c = heard.bed_c.or(h.bed_c);
                h.bed_target_c = heard.bed_target_c.or(h.bed_target_c);
                h.nozzle_heating = heard.nozzle_heating.or(h.nozzle_heating);
                h.bed_heating = heard.bed_heating.or(h.bed_heating);
            };
            let mut on = |event: Event| {
                if let Event::Said(text) = event {
                    heard(&state, &text);
                    if !text.starts_with("T:") && !text.starts_with(" T:") {
                        state.lock().unwrap().said.push(text);
                    }
                }
            };
            // Temperatures reported by the printer every 2 s while it prints (where it
            // can; a printer that cannot just says so).
            let _ = printer.send_line("M155 S2", &mut on);
            let mut end = JobEnd {
                id,
                ok: true,
                stopped: false,
                reason: None,
                restart_line: None,
            };
            for (n, code) in codes.iter().enumerate() {
                // Paused: nothing more is sent until resumed (or stopped).
                loop {
                    let (paused, stop) = {
                        let s = state.lock().unwrap();
                        (s.paused, s.stop)
                    };
                    if stop || !paused {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                if state.lock().unwrap().stop {
                    end.ok = false;
                    end.stopped = true;
                    for code in STOP {
                        let _ = printer.send_line(code, &mut on);
                    }
                    break;
                }
                match printer.send_line(code, &mut on) {
                    Ok(ok) => heard(&state, &ok),
                    Err(error) => {
                        end.ok = false;
                        end.reason = Some(format!("at line {} of the file: {error}", n + 1));
                        break;
                    }
                }
                state.lock().unwrap().done = n + 1;
            }
            let _ = printer.send_line("M155 S0", &mut on);
            (printer, end)
        });
        Self {
            id,
            name: name.to_owned(),
            shared,
            thread: Some(thread),
            started: std::time::Instant::now(),
        }
    }

    pub fn pause(&self, paused: bool) {
        self.shared.lock().unwrap().paused = paused;
    }

    pub fn stop(&self) {
        let mut s = self.shared.lock().unwrap();
        s.stop = true;
        s.paused = false;
    }

    pub fn progress(&self) -> Progress {
        let mut s = self.shared.lock().unwrap();
        Progress {
            done: s.done,
            total: s.total,
            paused: s.paused,
            heat: s.heat,
            elapsed_s: self.started.elapsed().as_secs(),
            said: std::mem::take(&mut s.said),
        }
    }

    /// The printer back and how the job ended, once it has: None while it runs.
    pub fn finished(&mut self) -> Option<(Printer<L>, JobEnd)> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        self.thread.take()?.join().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marlin::tests::FakeMarlin;

    fn connected(accepted: Arc<Mutex<Vec<String>>>) -> Printer<FakeMarlin> {
        let mut printer = Printer::new(FakeMarlin::new(accepted));
        printer.connect(&mut |_| {}).unwrap();
        printer
    }

    fn wait(job: &mut PrintJob<FakeMarlin>) -> (Printer<FakeMarlin>, JobEnd) {
        for _ in 0..500 {
            if let Some(done) = job.finished() {
                return done;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the print did not end");
    }

    /// The file goes to the printer in order, its temperatures are read on the way, and
    /// the printer comes back when it is done.
    #[test]
    fn a_print_runs_to_its_end() {
        let accepted = Arc::default();
        let gcode = "; a cube\nG28\nM104 S210\nG1 X10 Y10 E1\nG1 X20 Y10 E2\n".to_owned();
        let mut job = PrintJob::start(7, "cube.gcode", gcode, connected(Arc::clone(&accepted)));
        let (_, end) = wait(&mut job);
        assert!(end.ok, "{end:?}");
        assert_eq!(end.id, 7);
        let sent = accepted.lock().unwrap().clone();
        assert_eq!(
            sent,
            [
                "M155 S2",
                "G28",
                "M104 S210",
                "G1 X10 Y10 E1",
                "G1 X20 Y10 E2",
                "M155 S0"
            ]
        );
        let progress = job.progress();
        assert_eq!((progress.done, progress.total), (4, 4));
        assert_eq!(progress.heat.nozzle_c, Some(210.0));
    }

    /// Stopped: nothing more of the file, heaters and motors off.
    #[test]
    fn a_stopped_print_turns_the_heaters_off() {
        let accepted = Arc::default();
        let gcode: String = (0..2000).map(|i| format!("G1 X{} Y1\n", i % 100)).collect();
        let mut job = PrintJob::start(8, "long.gcode", gcode, connected(Arc::clone(&accepted)));
        job.stop();
        let (_, end) = wait(&mut job);
        assert!(end.stopped && !end.ok);
        let sent = accepted.lock().unwrap().clone();
        assert!(sent.len() < 100, "stopped early: {} lines", sent.len());
        assert!(sent.ends_with(&[
            "M104 S0".to_owned(),
            "M140 S0".to_owned(),
            "M107".to_owned(),
            "M84".to_owned(),
            "M155 S0".to_owned()
        ]));
    }
}
