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
    /// An `Error:` about the print (not about the line on the wire): the job stops.
    error: Option<String>,
}

/// A print running.
pub struct PrintJob<L: Read + Write + Send + 'static> {
    pub id: i64,
    pub name: String,
    /// What is on the printer's screen: waiting for someone (an `M0`, a filament change), a
    /// question, an error, paused or stopped at its knob.
    pub waiting: Arc<marlin::Screen>,
    shared: Arc<Mutex<Shared>>,
    thread: Option<JoinHandle<(Printer<L>, JobEnd)>>,
    started: std::time::Instant,
    /// Breaks off a heat-up the printer is waiting for, where it can.
    interrupt: Arc<std::sync::atomic::AtomicBool>,
    /// A button pressed for the question on its screen, sent while the job runs.
    answer: Arc<Mutex<Option<u8>>>,
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

/// How a job begins: kept for a power cut (crate::recovery), and from where in its file.
#[derive(Debug, Clone, Default)]
pub struct Begin {
    /// Where its checkpoint and a copy of its G-code are kept while it runs.
    pub recovery: Option<crate::recovery::Recovery>,
    /// The first of its code lines to send (comments left out); 0 from the start.
    pub from_index: usize,
    /// Taken up after a power cut: the printer heated and moved back itself and set its line
    /// number (the caller set the printer's to match); its print timer runs on.
    pub resumed: bool,
}

/// How often a running job's checkpoint is written at least.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(2);

impl<L: Read + Write + Send + 'static> PrintJob<L> {
    /// Starts streaming `gcode` to the printer, which the job holds until it ends.
    #[cfg(test)]
    pub fn start(id: i64, name: &str, gcode: String, printer: Printer<L>) -> Self {
        Self::start_with(id, name, gcode, printer, Begin::default())
    }

    /// The same, kept for a power cut and from where `begin` says.
    pub fn start_with(
        id: i64,
        name: &str,
        gcode: String,
        mut printer: Printer<L>,
        begin: Begin,
    ) -> Self {
        let waiting = printer.waiting();
        let interrupt = printer.interrupt.clone();
        let answer = printer.answer.clone();
        // A stop from its menu before this job is not for this job.
        waiting.clear_cancel();
        let screen = waiting.clone();
        let codes: Vec<String> = gcode
            .lines()
            .filter_map(marlin::clean)
            .map(str::to_owned)
            .collect();
        let shared = Arc::new(Mutex::new(Shared {
            total: codes.len(),
            done: begin.from_index.min(codes.len()),
            ..Shared::default()
        }));
        let state = shared.clone();
        let job_name = name.to_owned();
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
                h.hotend_fan_rpm = heard.hotend_fan_rpm.or(h.hotend_fan_rpm);
                h.print_fan_rpm = heard.print_fan_rpm.or(h.print_fan_rpm);
            };
            let mut on = |event: Event| match event {
                Event::Said(text) => {
                    heard(&state, &text);
                    if !marlin::routine(&text) {
                        state.lock().unwrap().said.push(text);
                    }
                }
                Event::Error(text) => {
                    state.lock().unwrap().error.get_or_insert(text);
                }
                _ => {}
            };
            // Prusa's MK3 firmware parks the head itself to pause, resume and stop (M601,
            // M602, M603: as PrusaLink does); elsewhere the host stops sending.
            let mk3 = printer.is_prusa_mk3();
            // Temperatures reported by the printer every 2 s while it prints, where it can
            // (its M115 said so, or said nothing about it).
            let autoreport = printer.caps.autoreport_temp || printer.caps.all.is_empty() || mk3;
            // Prusa's MK3 reports fans and position too (C: temperatures 1, fans 2,
            // position 4; as PrusaLink asks).
            let (report_on, report_off) = if mk3 {
                ("M155 S2 C7", "M155 S0 C0")
            } else {
                ("M155 S2", "M155 S0")
            };
            if autoreport {
                let _ = printer.send_line(report_on, &mut on);
            }
            // Prusa's MK3 keeps its print timer and statistics by these (as PrusaLink does):
            // M75 starts it, M76 pauses, M77 stops. Taken up after a power cut it runs on.
            if mk3 && !begin.resumed {
                let _ = printer.send_line("M75", &mut on);
            }
            // Kept for a power cut: the G-code, and where the next of its lines goes.
            let checkpoint = |printer: &Printer<L>, index: usize| crate::recovery::Checkpoint {
                job_id: id,
                name: job_name.clone(),
                number: printer.next_number(),
                index,
                total: codes.len(),
                extras: crate::recovery::Checkpoint::keep_extras(printer.extras.clone()),
            };
            if let Some(recovery) = &begin.recovery
                && let Err(error) = recovery.begin(&gcode, &checkpoint(&printer, begin.from_index))
            {
                state
                    .lock()
                    .unwrap()
                    .said
                    .push(format!("not kept for a power cut: {error}"));
            }
            let mut saved_at = std::time::Instant::now();
            let mut saved_extras = printer.extras.len();
            let mut end = JobEnd {
                id,
                ok: true,
                stopped: false,
                reason: None,
                restart_line: None,
            };
            // Stopped by someone: heaters, fan and motors off (or Prusa's own stop, which
            // also lifts and parks).
            let stop_printer = |printer: &mut Printer<L>, on: &mut dyn FnMut(Event)| {
                let mut on = |e| on(e);
                if mk3 {
                    let _ = printer.send_line("M77", &mut on);
                    let _ = printer.send_line("M603", &mut on);
                } else {
                    for code in STOP {
                        let _ = printer.send_line(code, &mut on);
                    }
                }
            };
            let mut parked = false;
            let mut restarted = false;
            for (n, code) in codes.iter().enumerate().skip(begin.from_index) {
                // Paused: nothing more is sent until resumed (or stopped).
                loop {
                    // Paused or stopped here, or at the printer's own knob.
                    let at_printer = screen.now();
                    let (paused, stop, asked) = {
                        let s = state.lock().unwrap();
                        (
                            s.paused || at_printer.paused,
                            s.stop || at_printer.cancelled,
                            s.paused,
                        )
                    };
                    // Paused from here on an MK3: it parks the head (and back at resume), its
                    // timer paused.
                    if mk3 && asked && !parked && !stop {
                        let _ = printer.send_line("M601", &mut on);
                        let _ = printer.send_line("M76", &mut on);
                        parked = true;
                    }
                    if stop || !paused {
                        break;
                    }
                    // Its question answered from the website while it waits.
                    let _ = printer.send_answer(&mut on);
                    std::thread::sleep(Duration::from_millis(200));
                }
                if parked && !state.lock().unwrap().stop {
                    let _ = printer.send_line("M602", &mut on);
                    let _ = printer.send_line("M75", &mut on);
                    parked = false;
                }
                if screen.now().cancelled {
                    // Stopped from its own menu: it stopped itself; nothing more goes.
                    screen.clear_cancel();
                    end.ok = false;
                    end.stopped = true;
                    end.reason = Some("stopped at the printer".to_owned());
                    break;
                }
                if state.lock().unwrap().stop {
                    end.ok = false;
                    end.stopped = true;
                    stop_printer(&mut printer, &mut on);
                    break;
                }
                // It stopped or was killed (its fault), or said an error about the print:
                // the job ends, as OctoPrint's does; heaters off where it still listens.
                let fault = screen.now().fault;
                let error = state.lock().unwrap().error.take();
                if let Some(why) = fault.clone().or(error) {
                    end.ok = false;
                    end.reason = Some(format!("at line {} of the file, the printer said: {why}", n));
                    if fault.is_none() {
                        stop_printer(&mut printer, &mut on);
                    }
                    break;
                }
                // Prusa's MK3: a host is still there (its menu and screen depend on it).
                let _ = printer.keep_host_alive(&mut on);
                // The checkpoint, after a line that was not the file's (the numbers and the
                // file's lines no longer go one for one there) and every two seconds.
                if let Some(recovery) = &begin.recovery
                    && (printer.extras.len() != saved_extras || saved_at.elapsed() >= CHECKPOINT_EVERY)
                {
                    let _ = recovery.save(&checkpoint(&printer, n));
                    saved_at = std::time::Instant::now();
                    saved_extras = printer.extras.len();
                }
                match printer.send_file_line(code, &mut on) {
                    Ok(ok) => heard(&state, &ok),
                    Err(error) => {
                        end.ok = false;
                        // Restarted (a power cut, on Prusa's MK3 to be taken up): its
                        // checkpoint stays.
                        restarted = error.to_string().contains("restarted");
                        end.reason = Some(format!("at line {} of the file: {error}", n + 1));
                        break;
                    }
                }
                state.lock().unwrap().done = n + 1;
                // A pause in the file itself (Prusa's MK3: M601, a colour change): it parks
                // and waits for someone; nothing more goes until it says it resumed.
                if mk3 && code.split_whitespace().next() == Some("M601") {
                    screen.paused_by_file();
                }
            }
            // Prusa's MK3: its print timer stops.
            if mk3 && !end.stopped && !restarted {
                let _ = printer.send_line("M77", &mut on);
            }
            // Ended (done, stopped, failed): nothing to take up after a power cut, unless the
            // printer restarted in the middle of it.
            if let Some(recovery) = &begin.recovery
                && !(restarted && mk3)
            {
                recovery.clear();
            }
            // The last line's error, said after its `ok`.
            if end.ok
                && let Some(why) = screen.now().fault.or(state.lock().unwrap().error.take())
            {
                end.ok = false;
                end.reason = Some(format!("at the end of the file, the printer said: {why}"));
            }
            if autoreport {
                let _ = printer.send_line(report_off, &mut on);
            }
            (printer, end)
        });
        Self {
            id,
            name: name.to_owned(),
            waiting,
            shared,
            thread: Some(thread),
            started: std::time::Instant::now(),
            interrupt,
            answer,
        }
    }

    /// Someone pressed a button of the question on its screen (a runout, a filament change).
    pub fn answer(&self, button: u8) {
        *self.answer.lock().unwrap() = Some(button);
    }

    pub fn pause(&self, paused: bool) {
        self.shared.lock().unwrap().paused = paused;
    }

    /// Stops the job: at once, if the printer waits for a heat-up it can break off.
    pub fn stop(&self) {
        let mut s = self.shared.lock().unwrap();
        s.stop = true;
        s.paused = false;
        self.interrupt
            .store(true, std::sync::atomic::Ordering::Relaxed);
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

    fn with(fake: FakeMarlin, m115: bool) -> Printer<FakeMarlin> {
        let mut printer = Printer::new(fake);
        printer.hurry();
        printer.connect(&mut |_| {}).unwrap();
        if m115 {
            printer.command("M115").unwrap();
        }
        printer
    }

    const MARLIN: &str = "FIRMWARE_NAME:Marlin 2.1.2.1 (Jun 1 2024) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Ender-3 EXTRUDER_COUNT:1\nCap:EMERGENCY_PARSER:1\nCap:AUTOREPORT_TEMP:0";

    /// An error about the print (not about the line on the wire) ends it, heaters off, as
    /// OctoPrint does; an unknown code does not.
    #[test]
    fn an_error_ends_the_print() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.errors = vec![
            ("M42".into(), "echo:Unknown command: \"M42\"".into()),
            ("G29".into(), "Error:Probing Failed".into()),
        ];
        let gcode = "G28\nM42\nG29\nG1 X1\nG1 X2\n".to_owned();
        let mut job = PrintJob::start(9, "x.gcode", gcode, with(fake, false));
        let (_, end) = wait(&mut job);
        assert!(!end.ok && !end.stopped, "{end:?}");
        assert!(end.reason.as_deref().unwrap().contains("Probing Failed"), "{end:?}");
        let sent = accepted.lock().unwrap().clone();
        assert!(!sent.contains(&"G1 X1".to_owned()), "{sent:?}");
        assert!(sent.contains(&"M104 S0".to_owned()), "heaters off: {sent:?}");
    }

    /// The printer stopped itself (a thermal runaway): the job ends, nothing more is sent.
    #[test]
    fn a_fault_ends_the_print() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.errors = vec![(
            "G1 X1".into(),
            "Error:Thermal Runaway, system stopped! Heater_ID: 0".into(),
        )];
        let gcode = "G28\nG1 X1\nG1 X2\nG1 X3\n".to_owned();
        let mut job = PrintJob::start(10, "x.gcode", gcode, with(fake, false));
        let (_, end) = wait(&mut job);
        assert!(!end.ok);
        assert!(end.reason.as_deref().unwrap().contains("Thermal Runaway"));
        assert!(!accepted.lock().unwrap().contains(&"G1 X2".to_owned()));
    }

    /// Stopped while it waits for a heat-up: with the emergency parser an M108 breaks it
    /// off at once; then heaters off. M155 is not sent where M115 said it cannot.
    #[test]
    fn a_stop_breaks_off_a_heat_up() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.m115 = MARLIN.into();
        fake.heat_until_m108 = true;
        let gcode = "M109 S215\nG28\nG1 X1\n".to_owned();
        let mut job = PrintJob::start(11, "x.gcode", gcode, with(fake, true));
        std::thread::sleep(Duration::from_millis(50));
        job.stop();
        let (_, end) = wait(&mut job);
        assert!(end.stopped);
        let sent = accepted.lock().unwrap().clone();
        assert_eq!(
            sent,
            ["M115", "M109 S215", "M108", "M104 S0", "M140 S0", "M107", "M84"],
            "no M155 (it cannot), the heat-up broken off, nothing more of the file"
        );
    }

    /// A question on its screen in the middle of a job (a runout), answered from the website:
    /// M876 with the button's index goes while the line that asked still waits, and the job
    /// goes on.
    #[test]
    fn a_question_is_answered_during_a_job() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.m115 = MARLIN.into();
        fake.asks_at = Some("M600".into());
        let gcode = "G1 X1\nM600\nG1 X2\n".to_owned();
        let mut job = PrintJob::start(14, "x.gcode", gcode, with(fake, true));
        // Asked: the question and its buttons are on its screen.
        let mut asked = None;
        for _ in 0..100 {
            asked = job.waiting.now().asking;
            if asked.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (question, buttons) = asked.expect("it asks");
        assert_eq!(question, "Filament runout");
        assert_eq!(buttons, ["Continue", "Purge more"]);
        job.answer(1);
        let (_, end) = wait(&mut job);
        assert!(end.ok, "{end:?}");
        let sent = accepted.lock().unwrap().clone();
        assert_eq!(sent, ["M115", "G1 X1", "M600", "M876 S1", "G1 X2"]);
        assert_eq!(job.waiting.now().asking, None, "answered");
    }

    /// Prusa's MK3 and a power cut: the job's checkpoint and G-code stay when the printer
    /// restarts in the middle; started again and resumed (M602) it asks for the line after the
    /// last one it carried out, and the job goes on from the line of its file that goes with it,
    /// numbered from there, without starting its timer again; done, nothing stays.
    #[test]
    fn an_mk3_print_is_taken_up_after_a_power_cut() {
        use crate::recovery::Recovery;
        let dir = std::env::temp_dir().join(format!("mc-pp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let recovery = Recovery::beside(&dir.join("agent.json"));
        let gcode: String = (1..=8).map(|i| format!("G1 X{i}\n")).collect();

        // Printing; the power goes after G1 X5.
        let mut fake = FakeMarlin::new(Arc::default());
        fake.restart_after = Some("G1 X5".into());
        let numbers = fake.numbers.clone();
        let mut job = PrintJob::start_with(
            21,
            "x.gcode",
            gcode.clone(),
            with(fake, true),
            Begin {
                recovery: Some(recovery.clone()),
                ..Begin::default()
            },
        );
        let (_, end) = wait(&mut job);
        assert!(!end.ok, "{end:?}");
        let (cp, kept) = recovery.load().expect("kept for a power cut");
        assert_eq!(kept, gcode);
        // It had carried out up to G1 X3 (the rest still in its buffers): that line's number.
        let saved = numbers
            .lock()
            .unwrap()
            .iter()
            .find(|(_, c)| c == "G1 X3")
            .unwrap()
            .0;

        // Started again, resumed: it asks for the next line.
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut fake = FakeMarlin::new(accepted.clone());
        fake.recovering = Some(saved);
        let mut printer = with(fake, true);
        printer.send_line("M602", &mut |_| {}).unwrap();
        let line = printer.take_stray_resend().expect("it asks for its next line");
        assert_eq!(line, saved + 1);
        let index = cp.index_of(line).unwrap();
        assert_eq!(index, 3, "G1 X4");
        printer.set_next_number(line);
        let mut job = PrintJob::start_with(
            21,
            "x.gcode",
            kept,
            printer,
            Begin {
                recovery: Some(recovery.clone()),
                from_index: index,
                resumed: true,
            },
        );
        let (_, end) = wait(&mut job);
        assert!(end.ok, "{end:?}");
        let sent = accepted.lock().unwrap().clone();
        let file: Vec<&String> = sent.iter().filter(|c| c.starts_with("G1")).collect();
        assert_eq!(file, ["G1 X4", "G1 X5", "G1 X6", "G1 X7", "G1 X8"]);
        assert!(!sent.contains(&"M75".to_owned()), "its timer runs on: {sent:?}");
        assert!(sent.contains(&"M77".to_owned()));
        assert!(recovery.load().is_none(), "done: nothing kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Prusa's MK3: a pause in the file itself (M601, a colour change) stops the sending
    /// until it says it resumed; its print timer runs from M75 to M77.
    #[test]
    fn an_mk3_pause_in_the_file_is_waited_for() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let fake = FakeMarlin::new(accepted.clone());
        let gcode = "G1 X1\nM601\nG1 X2\nG1 X3\n".to_owned();
        let mut job = PrintJob::start(13, "x.gcode", gcode, with(fake, true));
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !accepted.lock().unwrap().contains(&"G1 X2".to_owned()),
            "waits after M601"
        );
        job.waiting.hear("//action:resumed");
        let (_, end) = wait(&mut job);
        assert!(end.ok, "{end:?}");
        let sent = accepted.lock().unwrap().clone();
        let at = |code: &str| sent.iter().position(|c| c == code).unwrap();
        assert!(at("M75") < at("G1 X1") && at("G1 X3") < at("M77"), "{sent:?}");
    }

    /// Prusa's MK3 firmware: paused with M601 (it parks), resumed with M602, stopped with
    /// M603 (it lifts and parks), as PrusaLink does.
    #[test]
    fn an_mk3_parks_itself() {
        let accepted: Arc<Mutex<Vec<String>>> = Arc::default();
        let fake = FakeMarlin::new(accepted.clone());
        // Long enough to still run after a pause and a resume.
        let gcode: String = (0..200_000).map(|i| format!("G1 X{} Y1\n", i % 100)).collect();
        let mut job = PrintJob::start(12, "x.gcode", gcode, with(fake, true));
        job.pause(true);
        std::thread::sleep(Duration::from_millis(300));
        job.pause(false);
        // Resumed for a while (the job looks every 200 ms), then stopped.
        std::thread::sleep(Duration::from_millis(400));
        job.stop();
        let (_, end) = wait(&mut job);
        assert!(end.stopped);
        let sent = accepted.lock().unwrap().clone();
        let at = |code: &str| sent.iter().position(|c| c == code);
        assert!(at("M601").is_some() && at("M602") > at("M601"), "{sent:?}");
        assert_eq!(sent[sent.len() - 2..], ["M603".to_owned(), "M155 S0 C0".to_owned()]);
        assert!(sent.contains(&"M155 S2 C7".to_owned()), "temperatures, fans, position");
        assert!(sent.contains(&"M79 S\"MC\"".to_owned()), "a host is there");
        assert!(!sent.contains(&"M104 S0".to_owned()));
    }
}
