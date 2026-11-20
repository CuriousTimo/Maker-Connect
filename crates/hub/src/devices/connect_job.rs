//! What a Buddy printer's print is doing, kept as Prusa's own server keeps it (`reference/prusa-connect`,
//! read only: OCL, nothing copied). The connector holds the print's word at `/job/state` beside the
//! machine's at `/state` (`internal/connector/xbuddy/jobrefresh.go`, `inbound.go` `noteJob`), and the
//! server's readers take a print as current from the two (`internal/app/devicejobs.go`
//! `jobStateOf`, `jobFacts.running`). Both words are in the telemetry ([`Telemetry::printer_state`],
//! [`Telemetry::job_state`]: the server's state tree); what the connector keeps per session (the
//! machine's last word, the job number, the word it last wrote) is in [`Job`], in memory with the
//! tool session ([`super::connect_tools::Session`]).
//!
//! Ours (the cost of our persistence): after a hub restart the tree is back and the session is
//! empty, as the server's is after a reconnect.
//!
//! Not covered: the refresh pump (SEND_JOB_INFO on a move, `requestJobRefresh`), `/job/outcome`
//! (`joboutcome.go`; a print's end is read off the machine's state, `Store::follow_own_print_of`),
//! the objects that go with a new job number (`forgetTheObjects`), and the job number on the tree
//! (`/job/id`, which a JOB_INFO restates, `inbound.go:394-400`: no reader here takes it).
//!
//! As the reference reads it: a printer that says PRINTING before it names a job, and then BUSY,
//! has no print under the BUSY (no job state was written, `noteThePrintIsRunning`). Buddy names the
//! job on every frame while it prints (`render.cpp`), so this stays the reference's rule.

use maker_connect_protocol::Telemetry;

/// `jobrefresh.go:84-89`, `inbound.go` `stateIdle`, `stateReady`; `devicejobs.go` `stateBusy`,
/// `stateAttention`.
const PRINTING: &str = "PRINTING";
const PAUSED: &str = "PAUSED";
const STOPPED: &str = "STOPPED";
const FINISHED: &str = "FINISHED";
const ATTENTION: &str = "ATTENTION";
const IDLE: &str = "IDLE";
const READY: &str = "READY";
const BUSY: &str = "BUSY";

/// `machineIsHoldingAPrint` (`jobrefresh.go:102`); the same words are the server's
/// `jobRunningStates` (`devicejobs.go:95`).
const HOLDING_A_PRINT: [&str; 3] = [PRINTING, PAUSED, ATTENTION];
/// `machineEndedThePrint` (`jobrefresh.go:109`).
const ENDED_THE_PRINT: [&str; 2] = [STOPPED, FINISHED];
/// `machineOverrulesTheJob` (`devicejobs.go:141`).
const OVERRULES_THE_JOB: [&str; 4] = [PRINTING, PAUSED, STOPPED, FINISHED];

/// The connector's per-session job facts (`Session` fields `machineState`, `job`, `jobState`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Job {
    /// The machine's last word (`machineState`); None before any.
    machine_state: Option<String>,
    /// The job number the machine last named (`job`); None is `jobIDUnknown`.
    job: Option<i64>,
    /// The word this session last wrote to `/job/state` (`jobState`); None before it wrote one.
    job_state: Option<String>,
    /// What was last written to `/job/state` and is not on the tree yet: put on it with the next
    /// telemetry frame (ours: the tree lives in the telemetry, which an event does not write).
    /// `Some(None)`: removed (a patch of null).
    pending: Option<Option<String>>,
    /// The machine's word an event wrote to `/state`, put on the tree with the next telemetry frame
    /// before that frame's own (ours, as `pending`).
    pending_state: Option<String>,
}

impl Job {
    /// A telemetry frame (`dispatchTelemetry`, `inbound.go:268-311`): its `/state` written, then its
    /// job number noted, then its state.
    pub(crate) fn telemetry(&mut self, t: &mut Telemetry, job_id: Option<i64>, state: &str) {
        if let Some(word) = self.pending_state.take() {
            t.printer_state = Some(word);
        }
        if !state.is_empty() {
            t.printer_state = Some(state.to_owned());
        }
        if let Some(id) = job_id {
            self.note_job_id(id);
        }
        self.note_machine_state(state);
        if let Some(word) = self.pending.take() {
            t.job_state = word;
        }
    }

    /// An event (`inbound.go:245-258`): a JOB_INFO's job (`noteJob`), then the envelope's state
    /// written to `/state` (`reportText` writes nothing for an empty one) and noted.
    pub(crate) fn event(&mut self, name: &str, state: &str, data: &serde_json::Value) {
        if name == "JOB_INFO" {
            self.note_job(state, data);
        }
        if !state.is_empty() {
            self.pending_state = Some(state.to_owned());
        }
        self.note_machine_state(state);
    }

    /// `noteJob` (`inbound.go:350-410`).
    fn note_job(&mut self, envelope: &str, data: &serde_json::Value) {
        if !job_frame(data) {
            return;
        }
        let mut state = data["state"].as_str().unwrap_or("").to_owned();
        if state.is_empty() {
            state = envelope.to_owned();
        }
        let machine = self.machine_word(envelope);
        if HOLDING_A_PRINT.contains(&machine.as_str()) {
            state.clear();
            if machine != ATTENTION {
                state = machine;
            }
        } else if self.machine_says_nothing_about_the_print(&machine) {
            state.clear();
        }
        if let Some(id) = data["job_id"].as_i64() {
            self.note_job_id(id);
        }
        if !self.job_state_holds_an_end() || HOLDING_A_PRINT.contains(&state.as_str()) {
            self.report_job_state(&state);
        }
    }

    /// `noteMachineState` (`jobrefresh.go:160-185`), without the refresh it asks for.
    fn note_machine_state(&mut self, state: &str) {
        if state.is_empty() {
            return;
        }
        let previous = self.machine_state.replace(state.to_owned());
        if previous.as_deref() == Some(state) {
            return;
        }
        let end = if self.print_was_abandoned(previous.as_deref(), state) {
            STOPPED
        } else {
            state
        };
        self.note_the_print_ended(end);
        self.note_the_print_is_running(state);
    }

    /// `printWasAbandoned` (`jobrefresh.go:198-210`).
    fn print_was_abandoned(&self, previous: Option<&str>, state: &str) -> bool {
        if !previous.is_some_and(|p| HOLDING_A_PRINT.contains(&p)) {
            return false;
        }
        if state != IDLE && state != READY {
            return false;
        }
        matches!(self.job_state.as_deref(), Some(PRINTING | PAUSED))
    }

    /// `noteThePrintEnded` (`jobrefresh.go:251-269`).
    fn note_the_print_ended(&mut self, machine: &str) {
        if !ENDED_THE_PRINT.contains(&machine) {
            return;
        }
        if self
            .job_state
            .as_deref()
            .is_some_and(|held| !HOLDING_A_PRINT.contains(&held))
        {
            return;
        }
        if self.job_state.is_none() && self.job.is_none() {
            return;
        }
        self.report_job_state(machine);
    }

    /// `noteThePrintIsRunning` (`jobrefresh.go:298-316`).
    fn note_the_print_is_running(&mut self, machine: &str) {
        if machine != PRINTING && machine != PAUSED {
            return;
        }
        if self.job_state_holds_an_end() {
            return;
        }
        if self.job_state.is_none() && self.job.is_none() {
            return;
        }
        self.report_job_state(machine);
    }

    /// `reportJobState` (`jobrefresh.go:319-326`); `reportText` writes nothing for an empty word.
    fn report_job_state(&mut self, state: &str) {
        if state.is_empty() {
            return;
        }
        self.job_state = Some(state.to_owned());
        self.pending = Some(Some(state.to_owned()));
    }

    /// `machineWord` (`jobrefresh.go:332-342`).
    fn machine_word(&self, carried: &str) -> String {
        if !carried.is_empty() {
            return carried.to_owned();
        }
        self.machine_state.clone().unwrap_or_default()
    }

    /// `machineSaysNothingAboutThePrint` (`jobrefresh.go:370-376`).
    fn machine_says_nothing_about_the_print(&self, machine: &str) -> bool {
        if machine.is_empty() || ENDED_THE_PRINT.contains(&machine) {
            return false;
        }
        self.job.is_some()
    }

    /// `jobStateHoldsAnEnd` (`jobrefresh.go:380-384`).
    fn job_state_holds_an_end(&self) -> bool {
        self.job_state
            .as_deref()
            .is_some_and(|held| ENDED_THE_PRINT.contains(&held))
    }

    /// `noteJobID` (`jobrefresh.go:410-428`), without the refresh and the objects: a new number
    /// retires the word the print before ended with (`forgetHowThePrintEnded`, `:437-440`).
    fn note_job_id(&mut self, id: i64) {
        if self.job.replace(id) == Some(id) {
            return;
        }
        if id > 0 && self.job_state_holds_an_end() {
            self.job_state = None;
            self.pending = Some(None);
        }
    }
}

/// Whether a JOB_INFO's data reads as a `jobFrame` (`inbound.go:330-336`): `json.Unmarshal` fails,
/// and `noteJob` writes nothing (`:355-359`), for data that is not an object or a member of the
/// wrong type. A null or absent member, and absent data (`len(reported.Data) > 0`), read as none.
fn job_frame(data: &serde_json::Value) -> bool {
    use serde_json::Value;
    let Some(frame) = data.as_object() else {
        return data.is_null();
    };
    let text = |key: &str| matches!(frame.get(key), None | Some(Value::Null | Value::String(_)));
    let number = match frame.get("job_id") {
        None | Some(Value::Null) => true,
        Some(n) => n.as_i64().is_some(),
    };
    number
        && ["state", "path", "display_name", "display"]
            .iter()
            .all(|k| text(k))
}

/// `jobStateOf` (`devicejobs.go:270-290`): the print's word, the machine's where the print's has
/// gone stale.
fn job_state_of(machine: &str, reported: &str) -> String {
    if reported.is_empty() {
        return machine.to_owned();
    }
    if HOLDING_A_PRINT.contains(&reported)
        && (OVERRULES_THE_JOB.contains(&machine) || ended(machine, ""))
    {
        return machine.to_owned();
    }
    if !HOLDING_A_PRINT.contains(&reported) && HOLDING_A_PRINT.contains(&machine) {
        return machine.to_owned();
    }
    reported.to_owned()
}

/// `jobFacts.running` (`devicejobs.go:325-331`).
fn running(machine: &str, job_state: &str) -> bool {
    if HOLDING_A_PRINT.contains(&machine) {
        return true;
    }
    machine == BUSY && HOLDING_A_PRINT.contains(&job_state)
}

/// `jobFacts.ended` (`devicejobs.go:366-368`).
fn ended(machine: &str, job_state: &str) -> bool {
    !machine.is_empty() && !running(machine, job_state) && machine != BUSY
}

/// Whether the print in the telemetry is the one on the bed (`jobFactsOf` then `running`).
pub(crate) fn is_running(t: &Telemetry) -> bool {
    let machine = t.printer_state.as_deref().unwrap_or("");
    let job_state = job_state_of(machine, t.job_state.as_deref().unwrap_or(""));
    running(machine, &job_state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(job: &mut Job, t: &mut Telemetry, state: &str, id: Option<i64>) -> bool {
        job.telemetry(t, id, state);
        is_running(t)
    }

    /// BUG-012's case on a MINI: bed levelling (BUSY) after a print ended is no print; a long move
    /// within a print is still the print.
    #[test]
    fn busy_counts_only_over_a_print_that_runs() {
        let mut job = Job::default();
        let mut t = Telemetry::default();
        assert!(frame(&mut job, &mut t, PRINTING, Some(29)));
        assert!(
            frame(&mut job, &mut t, BUSY, Some(29)),
            "a long move mid-print"
        );
        assert!(frame(&mut job, &mut t, PRINTING, Some(29)));
        assert!(!frame(&mut job, &mut t, FINISHED, Some(29)));
        assert_eq!(t.job_state.as_deref(), Some(FINISHED));
        assert!(!frame(&mut job, &mut t, IDLE, Some(0)));
        assert!(!frame(&mut job, &mut t, BUSY, None), "G29 after the print");
        assert_eq!(t.job_state.as_deref(), Some(FINISHED), "how it ended stays");
        // the next print retires the end
        assert!(frame(&mut job, &mut t, PRINTING, Some(30)));
        assert_eq!(t.job_state.as_deref(), Some(PRINTING));
    }

    /// A print cancelled at the printer goes PRINTING then IDLE: written as STOPPED.
    #[test]
    fn a_print_gone_idle_is_stopped() {
        let mut job = Job::default();
        let mut t = Telemetry::default();
        assert!(frame(&mut job, &mut t, PRINTING, Some(4)));
        assert!(!frame(&mut job, &mut t, IDLE, None));
        assert_eq!(t.job_state.as_deref(), Some(STOPPED));
        assert!(!frame(&mut job, &mut t, BUSY, None));
    }

    /// A state with no job named writes no job state; BUSY then counts as no print.
    #[test]
    fn no_job_named_no_job_state() {
        let mut job = Job::default();
        let mut t = Telemetry::default();
        assert!(
            frame(&mut job, &mut t, PRINTING, None),
            "PRINTING holds by itself"
        );
        assert_eq!(t.job_state, None);
        assert!(!frame(&mut job, &mut t, BUSY, None));
    }

    /// A JOB_INFO answered while a dialog stands says PAUSED; the machine's word decides, and
    /// ATTENTION writes none (BUG-013).
    #[test]
    fn job_info_takes_the_machines_word() {
        let mut job = Job::default();
        let mut t = Telemetry::default();
        frame(&mut job, &mut t, ATTENTION, Some(7));
        job.event(
            "JOB_INFO",
            ATTENTION,
            &serde_json::json!({"state": PAUSED, "job_id": 7}),
        );
        assert!(frame(&mut job, &mut t, PRINTING, Some(7)));
        assert_eq!(t.job_state.as_deref(), Some(PRINTING));
        assert!(
            frame(&mut job, &mut t, BUSY, Some(7)),
            "PRINTING held under BUSY"
        );
    }

    /// An event's state reaches `/state` with the next frame, before the frame's own; a JOB_INFO
    /// that does not read as a `jobFrame` writes nothing.
    #[test]
    fn an_event_writes_the_machines_word() {
        let mut job = Job::default();
        let mut t = Telemetry::default();
        frame(&mut job, &mut t, PRINTING, Some(5));
        job.event("STATE_CHANGED", PAUSED, &serde_json::Value::Null);
        job.telemetry(&mut t, None, "");
        assert_eq!(t.printer_state.as_deref(), Some(PAUSED));
        assert_eq!(t.job_state.as_deref(), Some(PAUSED));
        assert!(!job_frame(&serde_json::json!({"state": 3})));
        assert!(!job_frame(&serde_json::json!({"job_id": "7"})));
        assert!(!job_frame(&serde_json::json!([1])));
        assert!(job_frame(
            &serde_json::json!({"job_id": null, "path": "/usb/a.gcode"})
        ));
        assert!(job_frame(&serde_json::Value::Null));
    }

    /// The server's reading: a print that claims to run on a machine that is idle is over.
    #[test]
    fn a_stale_running_word_is_overruled() {
        let t = Telemetry {
            printer_state: Some(IDLE.into()),
            job_state: Some(PRINTING.into()),
            ..Telemetry::default()
        };
        assert!(!is_running(&t));
        let t = Telemetry {
            printer_state: Some(BUSY.into()),
            job_state: Some(PAUSED.into()),
            ..Telemetry::default()
        };
        assert!(is_running(&t));
    }
}
