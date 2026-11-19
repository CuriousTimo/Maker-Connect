//! Virtual lasers: LaserFoundry's simulated GRBL behind the very code a real laser on an agent
//! goes through (`maker-connect-grbl`), each on a thread of its own that does what an agent
//! does: report, take jobs and commands, say how they went. So a virtual laser homes, jogs,
//! frames, streams a job by character counting, stops and starts again as a real one would.

use crate::Store;
use crate::store::CommandState;
use maker_connect_grbl::{GrblMachine, SimulatedGrbl, commands_of};
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
pub fn ensure(store: &Store, id: &str, token: &str, travel: Option<[f64; 2]>) {
    if !running().lock().unwrap().insert(id.to_owned()) {
        return;
    }
    let (store, id, token) = (store.clone(), id.to_owned(), token.to_owned());
    // The thread waits for the store's Postgres side on the hub's runtime.
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        run(&runtime, &store, &id, &token, travel);
        running().lock().unwrap().remove(&id);
    });
}

fn run(
    runtime: &tokio::runtime::Handle,
    store: &Store,
    id: &str,
    token: &str,
    travel: Option<[f64; 2]>,
) {
    let mut sim = SimulatedGrbl::new();
    sim.set_latency(LINE_TIME);
    let mut laser = GrblMachine::new(Box::new(sim));
    if let Some([x, y]) = travel {
        let _ = carry_out(
            &mut laser,
            &format!("$130={x:.0}\n$131={y:.0}\n$$"),
            &mut None,
        );
    }
    let mut job_end: Option<JobEnd> = None;
    loop {
        std::thread::sleep(REPORT_EVERY);
        let update = laser.report();
        let (mut telemetry, ended) = (update.telemetry, update.ended);
        telemetry.firmware = Some("Grbl 1.1 (LaserFoundry's simulator)".to_owned());
        telemetry.job_end = ended.or(job_end.take());
        // What the hub has for it, with the store held only this long.
        let Ok(list) = runtime.block_on(store.virtual_machines()) else {
            continue;
        };
        if !list.iter().any(|(machine, ..)| machine == id) {
            return; // removed
        }
        let _ = runtime.block_on(store.report(token, &telemetry));
        let of = runtime
            .block_on(crate::agent_of(store, token))
            .unwrap_or_default();
        let (order, control) = runtime
            .block_on(crate::agent_job(store, &of, &telemetry))
            .unwrap_or_default();
        let caller = runtime.block_on(store.caller(token)).ok().flatten();
        let (order, control, command, file) = {
            let mut store = store.lock().unwrap();
            let command = if telemetry.job_id.is_none() && order.is_none() {
                store.next_command(caller.as_ref()).ok().flatten()
            } else {
                None
            };
            if let Some((command, _)) = &command {
                let _ = store.command_went(
                    caller.as_ref(),
                    *command,
                    CommandState::Accepted,
                    None,
                    Vec::new(),
                );
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
            let _ = laser.control(control);
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
            let (state, reason, output) = match carry_out(&mut laser, &gcode, &mut job_end) {
                Ok(said) => (CommandState::Finished, None, said),
                Err(why) => (CommandState::Rejected, Some(why), Vec::new()),
            };
            let _ =
                store
                    .lock()
                    .unwrap()
                    .command_went(caller.as_ref(), command, state, reason, output);
        }
    }
}

/// Hands a command's G-code to the laser and waits for its answers: the virtual laser's thread
/// is the hub's own simulator (the port itself never waits). A job that ends meanwhile is kept
/// in `ended` for the next report.
fn carry_out(
    laser: &mut GrblMachine,
    gcode: &str,
    ended: &mut Option<JobEnd>,
) -> Result<Vec<String>, String> {
    let ticket = laser.submit(&commands_of(gcode)?)?;
    loop {
        let update = laser.report();
        if update.ended.is_some() {
            *ended = update.ended;
        }
        if let Some(done) = update.done.into_iter().find(|d| d.ticket == ticket) {
            return done.result;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
