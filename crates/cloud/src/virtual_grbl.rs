//! Virtual lasers: LaserFoundry's simulated GRBL behind the very code a real laser on an agent
//! goes through (`maker-connect-grbl`), each on a thread of its own that does what an agent
//! does: report, take jobs and commands, say how they went. So a virtual laser homes, jogs,
//! frames, streams a job by character counting, stops and starts again as a real one would.

use crate::Shared;
use crate::store::CommandState;
use maker_connect_grbl::{GrblMachine, SimulatedGrbl};
use maker_connect_protocol::JobEnd;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How often a virtual laser reports, as an agent does while a job runs.
const REPORT_EVERY: Duration = Duration::from_millis(500);
/// The simulated controller's time per line, so a job takes a while to watch.
const LINE_TIME: Duration = Duration::from_millis(15);

/// The virtual lasers that have their thread.
fn running() -> &'static Mutex<HashSet<String>> {
    static RUNNING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// Starts a virtual laser's thread, unless it has one. `travel` sets its controller's travel
/// to the profile's work area.
pub fn ensure(store: &Shared, id: &str, token: &str, travel: Option<[f64; 2]>) {
    if !running().lock().unwrap().insert(id.to_owned()) {
        return;
    }
    let (store, id, token) = (store.clone(), id.to_owned(), token.to_owned());
    std::thread::spawn(move || {
        run(&store, &id, &token, travel);
        running().lock().unwrap().remove(&id);
    });
}

fn run(store: &Shared, id: &str, token: &str, travel: Option<[f64; 2]>) {
    let mut sim = SimulatedGrbl::new();
    sim.set_latency(LINE_TIME);
    let mut laser = GrblMachine::new(Box::new(sim));
    if let Some([x, y]) = travel {
        let _ = laser.carry_out(&format!("$130={x:.0}\n$131={y:.0}\n$$"));
    }
    let mut job_end: Option<JobEnd> = None;
    loop {
        std::thread::sleep(REPORT_EVERY);
        let (mut telemetry, ended) = laser.report();
        telemetry.firmware = Some("Grbl 1.1 (LaserFoundry's simulator)".to_owned());
        telemetry.job_end = ended.or(job_end.take());
        // What the cloud has for it, with the store held only this long.
        let (order, control, command, file) = {
            let mut store = store.lock().unwrap();
            let Ok(list) = store.virtual_machines() else {
                continue;
            };
            if !list.iter().any(|(machine, ..)| machine == id) {
                return; // removed
            }
            let _ = store.report(token, &telemetry);
            let (order, control) =
                crate::agent_job(&mut store, token, &telemetry).unwrap_or_default();
            let command = if telemetry.job_id.is_none() && order.is_none() {
                store.next_command(token).ok().flatten()
            } else {
                None
            };
            if let Some((command, _)) = &command {
                let _ =
                    store.command_went(token, *command, CommandState::Accepted, None, Vec::new());
            }
            let file = order.as_ref().and_then(|o| {
                let file = o
                    .download
                    .strip_prefix("/files/")?
                    .strip_suffix("/download")?;
                Some(store.file_path(file))
            });
            (order, control, command, file)
        };
        if let Some(control) = control {
            laser.control(control);
        }
        if let Some(order) = order {
            let started = file
                .ok_or_else(|| "no file".to_owned())
                .and_then(|path| std::fs::read_to_string(path).map_err(|e| e.to_string()))
                .and_then(|program| laser.start(order.id, &order.name, program, order.from_line));
            if let Err(reason) = started {
                job_end = Some(JobEnd {
                    id: order.id,
                    ok: false,
                    stopped: false,
                    reason: Some(reason),
                    restart_line: None,
                });
            }
        }
        if let Some((command, gcode)) = command {
            let (state, reason, output) = match laser.carry_out(&gcode) {
                Ok(said) => (CommandState::Finished, None, said),
                Err(why) => (CommandState::Rejected, Some(why), Vec::new()),
            };
            let _ = store
                .lock()
                .unwrap()
                .command_went(token, command, state, reason, output);
        }
    }
}
