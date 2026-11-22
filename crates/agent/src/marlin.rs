//! A Marlin printer on the agent's USB (`docs/design/prusa-sdk-extension.md`, part 9): the SDK as
//! PrusaLink hosts a printer on it (Prusa-Link at `reference/Prusa-Link`), each command handed to
//! the OctoPrint port (`maker_connect_engine::ports::octoprint`) as its `Action` or `Order`.
//!
//! PrusaLink's handlers are followed for what goes to the hub: the checks before a command and
//! their texts (`printer_adapter/command_handlers.py`), the telemetry keys
//! (`structures/model_classes.py:39-72`), and the end of a print (`state_manager.py:721-749`).
//! The states are part 9's mapping of OctoPrint's (`comm.py:445-460`).

use super::{Agent, set_state};
use maker_connect_engine::{Order, Report};
use maker_connect_gcode_metadata::py::repr_str;
use maker_connect_protocol::{Action, Heater, JobControl, JobOrder};
use maker_connect_sdk::command::{Command, CommandHandler, Finish, HandlerError};
use maker_connect_sdk::consts::{self, Source, State};
use maker_connect_sdk::models::Kwargs;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The agent's folder for print files, a `LOCAL` storage named, as PrusaLink names its own, by
/// the folder (`prusa_link.py:932-936`, `os.path.basename(path)`).
pub(crate) const STORAGE: &str = "gcodes";

/// `PRINT_END_TIMEOUT` (Prusa-Link `prusa/link/const.py:100`): `FINISHED` or `STOPPED` is held
/// at least this long after a print ends.
const PRINT_END_TIMEOUT: Duration = Duration::from_secs(11);

/// Part 9's table: OctoPrint's `MachineCom` state, by its name, as the protocol's `state`; none
/// for `NONE` and `CLOSED` (the printer is gone).
pub(crate) fn protocol_state(comm_state: Option<&str>) -> Option<State> {
    Some(match comm_state? {
        "STATE_OPEN_SERIAL"
        | "STATE_DETECT_SERIAL"
        | "STATE_CONNECTING"
        | "STATE_TRANSFERING_FILE" => State::Busy,
        "STATE_OPERATIONAL" => State::Idle,
        "STATE_STARTING" | "STATE_PRINTING" | "STATE_RESUMING" | "STATE_FINISHING"
        | "STATE_CANCELLING" => State::Printing,
        "STATE_PAUSING" | "STATE_PAUSED" => State::Paused,
        "STATE_ERROR" | "STATE_CLOSED_WITH_ERROR" => State::Error,
        _ => return None,
    })
}

/// Whether the printer is there: the port runs and OctoPrint's state is not `NONE` or `CLOSED`.
pub(crate) fn present(report: &Report) -> bool {
    report.connected && protocol_state(report.telemetry.comm_state.as_deref()).is_some()
}

/// The state the hub is told: an `ERROR` of the printer first (PrusaLink's override state), then
/// a print's end while it is held, then part 9's table.
pub(crate) fn state(agent: &Agent, report: Option<&Report>) -> State {
    let base = report
        .and_then(|r| protocol_state(r.telemetry.comm_state.as_deref()))
        .unwrap_or(State::Busy);
    if base == State::Error {
        return base;
    }
    match *agent.print_end.lock().unwrap() {
        Some((end, _)) => end,
        None => base,
    }
}

/// What the printer said to M115, as the port has it (part 9: `firmware`, `firmware_info`,
/// `firmware_capabilities` in `INFO`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Firmware {
    pub name: Option<String>,
    pub info: Option<serde_json::Map<String, serde_json::Value>>,
    pub capabilities: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Once a second, from the link loop: a print's end, and when it is held no longer; then a
/// `STATE_CHANGED` when the state is another than the last one said.
pub(crate) fn tick(agent: &Agent, report: &Report) {
    // Part 9: its M115 answer in `INFO`, sent again when it changed.
    let identity = report.telemetry.identity.as_ref();
    let firmware = Firmware {
        name: report.telemetry.firmware.clone(),
        info: identity.and_then(|i| i.firmware_info.clone()),
        capabilities: identity.and_then(|i| i.firmware_capabilities.clone()),
    };
    if firmware.name.is_some() && agent.firmware.lock().unwrap().as_ref() != Some(&firmware) {
        *agent.firmware.lock().unwrap() = Some(firmware);
        crate::info_changed(agent);
    }
    let job = agent.label.lock().unwrap().as_ref().map(|(job, _)| *job);
    if let Some(job) = job
        && let Some(end) = report.job_ends.iter().find(|e| e.id == job).cloned()
    {
        if let Some(id) = *agent.dymo.lock().unwrap() {
            let _ = agent
                .engine
                .lock()
                .unwrap()
                .reported(id, 0, report.job_ends.len());
        }
        log::info!(
            "print {job}: {}",
            if end.ok {
                "finished"
            } else {
                end.reason.as_deref().unwrap_or("stopped")
            }
        );
        *agent.label.lock().unwrap() = None;
        agent.printing.store(false, Ordering::SeqCst);
        // `finished()` and `stopped()` (`state_manager.py`): `print_ended_at = monotonic()`.
        let end_state = if end.ok {
            State::Finished
        } else {
            State::Stopped
        };
        *agent.print_end.lock().unwrap() = Some((end_state, Instant::now()));
    }
    // `instruction_confirmed` (`state_manager.py:721-749`): the end is let go once the timeout has
    // passed and the printer takes instructions again (OctoPrint's `OPERATIONAL`).
    {
        let mut print_end = agent.print_end.lock().unwrap();
        if let Some((_, at)) = *print_end
            && at.elapsed() > PRINT_END_TIMEOUT
            && protocol_state(report.telemetry.comm_state.as_deref()) == Some(State::Idle)
        {
            *print_end = None;
        }
    }
    let now = state(agent, Some(report));
    if *agent.last_state.lock().unwrap() != Some(now) {
        set_state(agent, now, report.telemetry.problem.as_deref());
    }
}

/// PrusaLink's telemetry keys that the port reads (part 9).
pub(crate) fn telemetry(t: &maker_connect_protocol::Telemetry) -> Kwargs {
    let mut kwargs = Kwargs::new();
    let mut put = |key: &str, v: Option<f32>| {
        if let Some(v) = v {
            kwargs.insert(key.into(), f64::from(v).into());
        }
    };
    put("temp_nozzle", t.nozzle_c);
    put("target_nozzle", t.nozzle_target_c);
    put("temp_bed", t.bed_c);
    put("target_bed", t.bed_target_c);
    if let Some([x, y, z]) = t.position_mm {
        put("axis_x", x);
        put("axis_y", y);
        put("axis_z", z);
    }
    if let Some(job) = &t.job {
        // `progress: Optional[int]`: the file's position in whole percent.
        if let Some(progress) = (job.done * 100).checked_div(job.total) {
            kwargs.insert("progress".into(), progress.into());
        }
        if let Some(s) = job.elapsed_s {
            kwargs.insert("time_printing".into(), s.into());
        }
    }
    kwargs
}

/// `CommandFailed(text)` as the SDK reports a handler's exception (`repr(err)`).
fn failed(text: &str) -> HandlerError {
    HandlerError::Failed(format!("CommandFailed({})", repr_str(text)))
}

type Handler = fn(&Agent, &Command) -> Result<Finish, HandlerError>;

/// The handlers PrusaLink sets (`prusa_link.py:203-217`), as far as part 9 has them.
pub(crate) fn setup(agent: &Arc<Agent>, command: &Command) {
    let handlers: [(consts::Command, Handler); 6] = [
        (consts::Command::StartPrint, start_print),
        (consts::Command::PausePrint, pause_print),
        (consts::Command::ResumePrint, resume_print),
        (consts::Command::StopPrint, stop_print),
        (consts::Command::Gcode, execute_gcode),
        (consts::Command::SetValue, set_value),
    ];
    for (name, handler) in handlers {
        let me = agent.clone();
        let h: CommandHandler = Arc::new(move |cmd| handler(&me, cmd));
        command.set_handler(name, h);
    }
}

fn current(agent: &Agent) -> State {
    let report = agent
        .dymo
        .lock()
        .unwrap()
        .and_then(|id| agent.engine.lock().unwrap().report(id).ok());
    state(agent, report.as_ref())
}

/// `start_print` (`prusa_link.py:563-570`): `assert caller.kwargs`, then `StartPrint`.
fn start_print(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let kwargs = cmd
        .kwargs()
        .filter(maker_connect_sdk::util::py_truthy)
        .ok_or_else(|| HandlerError::Failed("AssertionError()".into()))?;
    let Some(path) = kwargs.get("path").and_then(Value::as_str) else {
        return Err(HandlerError::Failed("KeyError('path')".into()));
    };
    start(agent, path, cmd.command_id())?;
    Ok(Finish::new(Source::Connect))
}

/// `StartPrint._run_command` (`command_handlers.py:257-303`) for a file of the agent's storage:
/// the file goes to the port as a job (OctoPrint streams it). The printer's SD card is not yet a
/// storage (part 9 has it; not yet here).
pub(crate) fn start(agent: &Agent, path: &str, job: i64) -> Result<(), HandlerError> {
    // "No new print jobs while already printing or when there is an Error/Attention state"
    if agent.label.lock().unwrap().is_some() || agent.print_end.lock().unwrap().is_some() {
        return Err(HandlerError::Failed(
            "NotStateToPrint('Already printing')".into(),
        ));
    }
    let now = current(agent);
    if matches!(now, State::Error | State::Attention) {
        return Err(HandlerError::Failed(format!(
            "NotStateToPrint({})",
            repr_str(&format!("Cannot print in State.{} state.", now.value()))
        )));
    }
    let not_found = || {
        HandlerError::Failed(format!(
            "FileNotFound({})",
            repr_str(&format!("The file at {path} does not exist."))
        ))
    };
    let file = path
        .strip_prefix(&format!("/{STORAGE}/"))
        .map(|rest| agent.dir.join(STORAGE).join(rest))
        .filter(|f| f.is_file())
        .ok_or_else(not_found)?;
    let Some(id) = *agent.dymo.lock().unwrap() else {
        return Err(failed("the Marlin module is off"));
    };
    let order = JobOrder {
        id: job,
        name: path.to_owned(),
        download: String::new(),
        size: std::fs::metadata(&file).map_or(0, |m| m.len()),
        from_line: None,
        tape_mm: None,
        label_length_mm: None,
        page_size: None,
    };
    *agent.label.lock().unwrap() = Some((job, file.clone()));
    agent.printing.store(true, Ordering::SeqCst);
    if agent
        .engine
        .lock()
        .unwrap()
        .send(id, Order::Job { order, file })
        .is_err()
    {
        *agent.label.lock().unwrap() = None;
        agent.printing.store(false, Ordering::SeqCst);
        return Err(failed("the Marlin port has ended"));
    }
    log::info!("print {job}: {path} to the printer");
    // `self.state_manager.printing()`
    set_state(agent, State::Printing, None);
    Ok(())
}

/// `download_finished_cb` (`prusa_link.py:519-540`): a download `to_print` is printed.
#[cfg(unix)]
pub(crate) fn downloaded(agent: &Agent, transfer: &maker_connect_sdk::download::Transfer) {
    let d = transfer.data().clone();
    if d.to_print != Some(true) {
        return;
    }
    if current(agent) == State::Attention {
        return;
    }
    let (Some(path), Some(job)) = (d.path.clone(), d.start_cmd_id) else {
        return;
    };
    if let Err(e) = start(agent, &path, job) {
        log::warn!("print {job}: not started: {e:?}");
    }
}

/// `PausePrint._run_command` (`command_handlers.py:214-226`): the port's `Pause`.
fn pause_print(agent: &Agent, _cmd: &Command) -> Result<Finish, HandlerError> {
    if current(agent) != State::Printing {
        return Err(failed("Cannot pause when not printing."));
    }
    control(agent, JobControl::Pause)
}

/// `ResumePrint._run_command` (`command_handlers.py:232-247`): the port's `Resume`.
fn resume_print(agent: &Agent, _cmd: &Command) -> Result<Finish, HandlerError> {
    if current(agent) != State::Paused {
        return Err(failed("Cannot resume when not paused."));
    }
    control(agent, JobControl::Resume)
}

/// `StopPrint._run_command` (`command_handlers.py:194-208`): the port's `Stop`.
fn stop_print(agent: &Agent, _cmd: &Command) -> Result<Finish, HandlerError> {
    control(agent, JobControl::Stop)
}

/// The job's pause, resume or stop to the port. PrusaLink's `TryUntilState` waits until the state
/// is reached; here the command ends when the port has it, the state follows by `STATE_CHANGED`.
fn control(agent: &Agent, control: JobControl) -> Result<Finish, HandlerError> {
    let Some(id) = *agent.dymo.lock().unwrap() else {
        return Err(failed("the Marlin module is off"));
    };
    agent
        .engine
        .lock()
        .unwrap()
        .send(id, Order::Control(control))
        .map_err(|_| failed("the Marlin port has ended"))?;
    Ok(Finish::new(Source::Connect))
}

/// `execute_gcode` (`prusa_link.py:545-552`) and `ExecuteGcode._run_command`
/// (`command_handlers.py:344-389`): the port's `Gcode`, as OctoPrint sends a typed command.
fn execute_gcode(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let gcode = cmd
        .kwargs()
        .and_then(|k| k.get("gcode").and_then(Value::as_str).map(str::to_owned))
        .ok_or_else(|| HandlerError::Failed("KeyError('gcode')".into()))?;
    let now = current(agent);
    if matches!(now, State::Printing | State::Attention | State::Error) {
        // PrusaLink's text, its stray `f` included (`command_handlers.py:353-354`).
        return Err(failed(&format!(
            "Can't run '{gcode}' while in f{} state.",
            now.value()
        )));
    }
    let output = run(agent, cmd.command_id(), Action::Gcode { text: gcode })?;
    Ok(gcode_finish(output))
}

/// `FINISHED` with what the printer answered, `gcode_output` in its `data` (extension part 9,
/// `gcode.output`); the SDK's own `FINISHED` when it answered nothing.
fn gcode_finish(output: Vec<String>) -> Finish {
    let mut finish = Finish::new(Source::Connect);
    if !output.is_empty() {
        finish.kwargs.insert("gcode_output".into(), output.into());
    }
    finish
}

/// `SetValue` (`command_handlers.py:395-449`): the checks but the ranges (part 9: PrusaLink's are
/// the MK3's), then the port's `Speed`, `Flow` or `Temperature`.
fn set_value(agent: &Agent, cmd: &Command) -> Result<Finish, HandlerError> {
    let params = match cmd.kwargs() {
        Some(Value::Object(map)) => map,
        _ => Default::default(),
    };
    if params.len() != 1 {
        return Err(failed("SET_VALUE needs exactly one value"));
    }
    let (key, value) = params.iter().next().unwrap();
    if !matches!(
        key.as_str(),
        "speed" | "flow" | "nozzle_temperature" | "bed_temperature"
    ) {
        return Err(failed(&format!("Can't set unknown value '{key}'")));
    }
    let Some(value) = value.as_i64().filter(|_| !value.is_f64()) else {
        return Err(failed(&format!("'{key}' has to be an integer")));
    };
    let now = current(agent);
    if !matches!(
        now,
        State::Idle
            | State::Paused
            | State::Finished
            | State::Stopped
            | State::Printing
            | State::Ready
    ) {
        return Err(failed(&format!(
            "Can't set '{key}' while in {} state.",
            now.value()
        )));
    }
    let percent =
        || u16::try_from(value).map_err(|_| failed(&format!("'{key}' has to be an integer")));
    let action = match key.as_str() {
        "speed" => Action::Speed {
            percent: percent()?,
        },
        "flow" => Action::Flow {
            percent: percent()?,
        },
        "nozzle_temperature" => Action::Temperature {
            heater: Heater::Nozzle,
            celsius: value as f64,
        },
        _ => Action::Temperature {
            heater: Heater::Bed,
            celsius: value as f64,
        },
    };
    run(agent, cmd.command_id(), action)?;
    Ok(Finish::new(Source::Connect))
}

/// An `Action` to the port, waited for as PrusaLink's `do_command` waits
/// (`command_queue.py:69-90`); the port's reason as the failure. What the printer said meanwhile.
fn run(agent: &Agent, command_id: i64, action: Action) -> Result<Vec<String>, HandlerError> {
    match crate::run_action(agent, command_id, action) {
        Ok(done) => done.map_err(|reason| failed(&reason)),
        Err(crate::Gone::Off) => Err(failed("the Marlin module is off")),
        Err(crate::Gone::Ended) => Err(failed(
            "Command has not been processed because the Marlin port has ended",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;
    use serde_json::json;

    /// A G-code's answer goes in `FINISHED`'s `data` as `gcode_output` (part 9); none, none.
    #[test]
    fn a_gcode_finishes_with_what_the_printer_answered() {
        let lines = vec![
            "FIRMWARE_NAME:Marlin 2.0.9.3 MACHINE_TYPE:AtomX MK3".to_owned(),
            "Cap:EEPROM:1".to_owned(),
            "ok".to_owned(),
        ];
        let finish = gcode_finish(lines.clone());
        assert_eq!(finish.source, Source::Connect);
        assert_eq!(
            finish.kwargs.get("gcode_output"),
            Some(&serde_json::json!(lines))
        );
        assert!(gcode_finish(Vec::new()).kwargs.is_empty());
    }

    #[test]
    fn the_states_of_part_9() {
        let cases = [
            ("STATE_NONE", None),
            ("STATE_OPEN_SERIAL", Some(State::Busy)),
            ("STATE_DETECT_SERIAL", Some(State::Busy)),
            ("STATE_CONNECTING", Some(State::Busy)),
            ("STATE_OPERATIONAL", Some(State::Idle)),
            ("STATE_STARTING", Some(State::Printing)),
            ("STATE_PRINTING", Some(State::Printing)),
            ("STATE_PAUSED", Some(State::Paused)),
            ("STATE_CLOSED", None),
            ("STATE_ERROR", Some(State::Error)),
            ("STATE_CLOSED_WITH_ERROR", Some(State::Error)),
            ("STATE_TRANSFERING_FILE", Some(State::Busy)),
            ("STATE_FINISHING", Some(State::Printing)),
            ("STATE_PAUSING", Some(State::Paused)),
            ("STATE_RESUMING", Some(State::Printing)),
            ("STATE_CANCELLING", Some(State::Printing)),
        ];
        // Every state OctoPrint names (`comm.py:445-460`), each once.
        let names: Vec<&str> =
            maker_connect_engine::ports::octoprint::comm::MachineCom::STATE_NAMES
                .iter()
                .map(|(name, _)| *name)
                .collect();
        assert_eq!(names.len(), cases.len());
        for (name, want) in cases {
            assert!(names.contains(&name), "{name}");
            assert_eq!(protocol_state(Some(name)), want, "{name}");
        }
        assert_eq!(protocol_state(None), None);
    }

    #[test]
    fn prusalinks_telemetry_keys() {
        let t = maker_connect_protocol::Telemetry {
            nozzle_c: Some(210.5),
            nozzle_target_c: Some(215.0),
            bed_c: Some(0.0),
            position_mm: Some([Some(1.0), Some(2.5), None]),
            job: Some(maker_connect_protocol::JobProgress {
                file: "/gcodes/a.gcode".into(),
                done: 999,
                total: 2000,
                elapsed_s: Some(61),
                remaining_s: None,
            }),
            ..Default::default()
        };
        let kw = telemetry(&t);
        let want = serde_json::json!({
            "temp_nozzle": 210.5, "target_nozzle": 215.0, "temp_bed": 0.0,
            "axis_x": 1.0, "axis_y": 2.5, "progress": 49, "time_printing": 61,
        });
        assert_eq!(Value::Object(kw), want);
        assert!(telemetry(&Default::default()).is_empty());
    }

    fn agent() -> Agent {
        let dir = std::env::temp_dir().join(format!("mc-marlin-{}", std::process::id()));
        Agent::new(
            Kind::Marlin,
            dir,
            crate::Settings::default(),
            Arc::new(std::sync::Mutex::new(maker_connect_engine::Engine::new())),
        )
    }

    fn report(comm: &str, ends: Vec<maker_connect_protocol::JobEnd>) -> Report {
        Report {
            telemetry: maker_connect_protocol::Telemetry {
                comm_state: Some(comm.into()),
                ..Default::default()
            },
            job_ends: ends,
            connected: true,
            ..Default::default()
        }
    }

    fn end(id: i64, ok: bool, stopped: bool) -> maker_connect_protocol::JobEnd {
        maker_connect_protocol::JobEnd {
            id,
            ok,
            stopped,
            reason: None,
            restart_line: None,
        }
    }

    /// A print's end as PrusaLink's (`state_manager.py:721-749`): `FINISHED` (or `STOPPED`) until
    /// `PRINT_END_TIMEOUT` has passed and the printer is operational, then `IDLE`; an `ERROR` first.
    #[test]
    fn a_prints_end_is_held() {
        for (ok, want) in [(true, State::Finished), (false, State::Stopped)] {
            let a = agent();
            *a.label.lock().unwrap() = Some((7, "/x".into()));
            a.printing.store(true, Ordering::SeqCst);
            tick(&a, &report("STATE_PRINTING", vec![]));
            assert_eq!(*a.last_state.lock().unwrap(), Some(State::Printing));

            tick(&a, &report("STATE_OPERATIONAL", vec![end(7, ok, !ok)]));
            assert_eq!(*a.last_state.lock().unwrap(), Some(want));
            assert!(a.label.lock().unwrap().is_none());
            assert!(!a.printing.load(Ordering::SeqCst));

            // within the timeout: held
            tick(&a, &report("STATE_OPERATIONAL", vec![]));
            assert_eq!(*a.last_state.lock().unwrap(), Some(want));
            // a printer's error goes first
            assert_eq!(
                state(&a, Some(&report("STATE_ERROR", vec![]))),
                State::Error
            );

            // past it, but not operational: held
            a.print_end.lock().unwrap().as_mut().unwrap().1 =
                Instant::now() - PRINT_END_TIMEOUT - Duration::from_secs(1);
            tick(&a, &report("STATE_CONNECTING", vec![]));
            assert_eq!(*a.last_state.lock().unwrap(), Some(want));
            // past it and operational: IDLE
            tick(&a, &report("STATE_OPERATIONAL", vec![]));
            assert_eq!(*a.last_state.lock().unwrap(), Some(State::Idle));
            assert!(a.print_end.lock().unwrap().is_none());
        }
    }

    #[test]
    fn gone_at_none_and_closed() {
        assert!(!present(&report("STATE_NONE", vec![])));
        assert!(!present(&report("STATE_CLOSED", vec![])));
        assert!(present(&report("STATE_CONNECTING", vec![])));
        let mut r = report("STATE_OPERATIONAL", vec![]);
        r.connected = false;
        assert!(!present(&r));
    }

    /// Part 9: what M115 said in `INFO`, which goes again when it changed
    /// (`_send_info_if_changed`, `printer_polling.py:1078-1083`), and only then.
    #[test]
    fn its_firmware_in_info() {
        let a = agent();
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = sent.clone();
        let cb: maker_connect_sdk::models::EventCallback =
            Arc::new(move |event, source, _, _, kwargs| {
                seen.lock().unwrap().push((event, source, kwargs));
            });
        *a.event_cb.lock().unwrap() = Some(cb);
        let said = |caps: Option<Value>| {
            let mut r = report("STATE_OPERATIONAL", vec![]);
            r.telemetry.firmware = Some("Marlin 2.0.9.3".into());
            r.telemetry.identity = Some(maker_connect_protocol::Identity {
                firmware_info: json!({"MACHINE_TYPE": "Crearibo CR10"})
                    .as_object()
                    .cloned(),
                firmware_capabilities: caps.and_then(|c| c.as_object().cloned()),
                ..Default::default()
            });
            r
        };
        let info = crate::send_info(&a).kwargs;
        assert_eq!(info["firmware"], crate::VERSION);
        assert!(!info.contains_key("firmware_info") && !info.contains_key("firmware_capabilities"));
        tick(&a, &report("STATE_OPERATIONAL", vec![]));
        tick(&a, &said(None));
        tick(&a, &said(None));
        tick(&a, &said(Some(json!({"AUTOLEVEL": true}))));
        let infos: Vec<_> = sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, ..)| *e == consts::Event::Info)
            .map(|(_, source, kwargs)| (*source, kwargs.get("firmware_capabilities").cloned()))
            .collect();
        assert_eq!(
            infos,
            [
                (Source::Connect, None),
                (Source::Connect, Some(json!({"AUTOLEVEL": true})))
            ]
        );
        let info = crate::send_info(&a).kwargs;
        assert_eq!(info["firmware"], "Marlin 2.0.9.3");
        assert_eq!(info["firmware_info"]["MACHINE_TYPE"], "Crearibo CR10");
    }

    #[test]
    fn its_type_and_folder() {
        assert_eq!(Kind::Marlin.printer_type(), "marlin.fdm");
        assert!(Kind::Marlin.extensions().contains(&"marlin.fdm"));
    }
}
