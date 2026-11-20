//! Command class representation (`command.py`).
//!
//! The Python object is shared by the WebSocket thread (`check_state`, `accept`) and the thread
//! that runs the command (`__call__`); here its fields sit behind one mutex, which is never held
//! while a callback runs.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::Value;

use crate::consts::{self, ONE_SECOND_TIMEOUT, PRIORITY_COMMANDS, Source};
use crate::models::{EventCallback, Kwargs};

/// `multiprocessing.Event`
#[derive(Debug, Default)]
pub struct Flag {
    set: Mutex<bool>,
    changed: Condvar,
}

impl Flag {
    pub fn new() -> Flag {
        Flag::default()
    }

    fn flag(&self) -> MutexGuard<'_, bool> {
        self.set.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn set(&self) {
        *self.flag() = true;
        self.changed.notify_all();
    }

    pub fn clear(&self) {
        *self.flag() = false;
    }

    pub fn is_set(&self) -> bool {
        *self.flag()
    }

    /// True if the flag is set, waiting up to `timeout` (forever when None).
    pub fn wait(&self, timeout: Option<Duration>) -> bool {
        let flag = self.flag();
        match timeout {
            None => *self
                .changed
                .wait_while(flag, |set| !*set)
                .unwrap_or_else(|p| p.into_inner()),
            Some(timeout) => {
                let (flag, _) = self
                    .changed
                    .wait_timeout_while(flag, timeout, |set| !*set)
                    .unwrap_or_else(|p| p.into_inner());
                *flag
            }
        }
    }
}

/// What a handler returns: the `**kwargs` of `Command.finish`, which means that source must be
/// set at least.
#[derive(Debug, Clone, PartialEq)]
pub struct Finish {
    pub source: Source,
    pub event: Option<consts::Event>,
    pub kwargs: Kwargs,
}

impl Finish {
    pub fn new(source: Source) -> Finish {
        Finish {
            source,
            event: None,
            kwargs: Kwargs::new(),
        }
    }
}

/// How a handler fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerError {
    /// `CommandRejected(reason)`: rejecting a command with an exact `reason`. The reason text
    /// is part of the contract with Connect, so it is sent as is in the REJECTED event.
    Rejected(String),
    /// Any other exception (`CommandFailed` among them), as its `repr(err)`.
    Failed(String),
}

pub type CommandHandler = Arc<dyn Fn(&Command) -> Result<Finish, HandlerError> + Send + Sync>;

#[derive(Debug, Clone)]
struct Fields {
    state: Option<consts::Event>,
    last_state: consts::Event,
    command_id: i64,
    command_name: Option<String>,
    force: bool,
    args: Option<Value>,
    kwargs: Option<Value>,
}

/// Command singleton/state like structure.
pub struct Command {
    pub event_cb: EventCallback,
    fields: Mutex<Fields>,
    handlers: Mutex<HashMap<consts::Command, CommandHandler>>,
    pub new_cmd_evt: Flag,
    pub cmd_end_evt: Flag,
    /// Called to stop the current command
    stop_cb: Mutex<Arc<dyn Fn() + Send + Sync>>,
}

fn kwargs_with(first: &[(&str, Value)], rest: Kwargs) -> Kwargs {
    let mut kwargs: Kwargs = first
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    kwargs.extend(rest);
    kwargs
}

impl Command {
    pub fn new(event_cb: EventCallback) -> Command {
        Command {
            event_cb,
            fields: Mutex::new(Fields {
                state: None,
                last_state: consts::Event::Rejected,
                command_id: -1, // -1 means that there was no command before
                command_name: None,
                force: false,
                args: Some(Value::Array(Vec::new())),
                kwargs: Some(Value::Object(Kwargs::new())),
            }),
            handlers: Mutex::new(HashMap::new()),
            new_cmd_evt: Flag::new(),
            cmd_end_evt: Flag::new(),
            stop_cb: Mutex::new(Arc::new(|| {})),
        }
    }

    fn fields(&self) -> MutexGuard<'_, Fields> {
        self.fields.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn state(&self) -> Option<consts::Event> {
        self.fields().state
    }

    pub fn last_state(&self) -> consts::Event {
        self.fields().last_state
    }

    pub fn command_id(&self) -> i64 {
        self.fields().command_id
    }

    pub fn command_name(&self) -> Option<String> {
        self.fields().command_name.clone()
    }

    pub fn force(&self) -> bool {
        self.fields().force
    }

    pub fn args(&self) -> Option<Value> {
        self.fields().args.clone()
    }

    pub fn kwargs(&self) -> Option<Value> {
        self.fields().kwargs.clone()
    }

    /// `command.state = …`
    pub fn set_state(&self, state: Option<consts::Event>) {
        self.fields().state = state;
    }

    /// `command.last_state = …`
    pub fn set_last_state(&self, last_state: consts::Event) {
        self.fields().last_state = last_state;
    }

    /// `command.command_id = …`
    pub fn set_command_id(&self, command_id: i64) {
        self.fields().command_id = command_id;
    }

    /// `command.command_name = …`
    pub fn set_command_name(&self, command_name: &str) {
        self.fields().command_name = Some(command_name.to_owned());
    }

    /// `command.handlers[command] = handler`
    pub fn set_handler(&self, command: consts::Command, handler: CommandHandler) {
        self.handlers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(command, handler);
    }

    pub fn set_stop_cb(&self, stop_cb: Arc<dyn Fn() + Send + Sync>) {
        *self.stop_cb.lock().unwrap_or_else(|p| p.into_inner()) = stop_cb;
    }

    fn emit(&self, event: consts::Event, source: Source, command_id: i64, kwargs: Kwargs) {
        (self.event_cb)(event, source, None, Some(command_id), kwargs);
    }

    /// Check if we're ready for another command
    ///
    /// Returns true, if command can be accepted. Otherwise, put right event to queue.
    pub fn check_state(&self, command_id: i64, command_name: &str) -> bool {
        if let Ok(command) = command_name.parse::<consts::Command>()
            && PRIORITY_COMMANDS.contains(&command)
        {
            let stop_cb = self
                .stop_cb
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            stop_cb();
            if !self
                .cmd_end_evt
                .wait(Some(Duration::from_secs(ONE_SECOND_TIMEOUT)))
            {
                log::warn!("Previous command didn't stop in time, ignoring {command_name}");
            }
            return true;
        }

        let fields = self.fields().clone();
        if let Some(state) = fields.state {
            // here comes another command
            if fields.command_id != command_id {
                let kwargs = kwargs_with(
                    &[
                        ("reason", "Another command is running".into()),
                        ("actual_command_id", fields.command_id.into()),
                    ],
                    Kwargs::new(),
                );
                self.emit(consts::Event::Rejected, Source::Connect, command_id, kwargs);
            } else {
                // resend state of accepted command
                self.emit(state, Source::Connect, command_id, Kwargs::new());
            }
            return false;
        }

        if fields.command_id == command_id {
            // resend last state of last_command
            self.emit(
                fields.last_state,
                Source::Connect,
                command_id,
                Kwargs::new(),
            );
            return false;
        }
        true
    }

    /// Accept command (add event to queue).
    pub fn accept(
        &self,
        command_id: i64,
        command_name: &str,
        args: Option<Value>,
        kwargs: Option<Value>,
        force: bool,
    ) {
        {
            let mut fields = self.fields();
            fields.state = Some(consts::Event::Accepted);
            fields.command_id = command_id;
            fields.command_name = Some(command_name.to_owned());
            fields.args = args;
            fields.kwargs = kwargs;
            fields.force = force;
        }
        self.emit(
            consts::Event::Accepted,
            Source::Connect,
            command_id,
            Kwargs::new(),
        );
        self.new_cmd_evt.set();
        self.cmd_end_evt.clear();
    }

    /// Reject command with some reason
    pub fn reject(&self, source: Source, reason: &str, kwargs: Kwargs) {
        let command_id = {
            let mut fields = self.fields();
            fields.last_state = consts::Event::Rejected;
            fields.command_id
        };
        let kwargs = kwargs_with(&[("reason", reason.into())], kwargs);
        self.emit(consts::Event::Rejected, source, command_id, kwargs);
        self.teardown();
        // don't clean data, which is history in fact
    }

    /// Notify Connect that a command has failed
    pub fn failed(&self, source: Source, reason: &str, command_id: Option<i64>, kwargs: Kwargs) {
        let command_id = {
            let mut fields = self.fields();
            fields.last_state = consts::Event::Failed;
            command_id.unwrap_or(fields.command_id)
        };
        let kwargs = kwargs_with(&[("reason", reason.into())], kwargs);
        self.emit(consts::Event::Failed, source, command_id, kwargs);
        self.teardown();
    }

    /// Finish command with optional other event and data.
    pub fn finish(
        &self,
        source: Source,
        event: Option<consts::Event>,
        command_id: Option<i64>,
        kwargs: Kwargs,
    ) {
        let event = event.unwrap_or(consts::Event::Finished);
        let command_id = {
            let mut fields = self.fields();
            fields.last_state = consts::Event::Finished;
            command_id.unwrap_or(fields.command_id)
        };
        self.emit(event, source, command_id, kwargs);
        self.teardown();
    }

    /// Clear the last command state and prepare to accept a new one
    pub fn teardown(&self) {
        self.fields().state = None;
        self.new_cmd_evt.clear();
        self.cmd_end_evt.set();
    }

    /// Run the command handler (`__call__`).
    pub fn run(&self) {
        let (state, command_id, command_name) = {
            let fields = self.fields();
            (
                fields.state,
                fields.command_id,
                fields.command_name.clone().unwrap_or_default(),
            )
        };
        if state != Some(consts::Event::Accepted) {
            return;
        }

        log::debug!("Try to handle {command_name} command.");
        // Remember the current command id during this specific command's run
        let Ok(cmd) = command_name.parse::<consts::Command>() else {
            log::error!("Unknown printer command {command_name}");
            return self.reject(Source::Wui, "Unknown command", Kwargs::new());
        };
        let handler = self
            .handlers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&cmd)
            .cloned();
        let Some(handler) = handler else {
            log::error!("Printer command {command_name} not implemented");
            return self.reject(Source::Wui, "Not Implemented", Kwargs::new());
        };
        let result = catch_unwind(AssertUnwindSafe(|| handler(self))).unwrap_or_else(|panic| {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(HandlerError::Failed(msg))
        });
        match result {
            Ok(finish) => self.finish(finish.source, finish.event, Some(command_id), finish.kwargs),
            Err(HandlerError::Rejected(reason)) => {
                self.reject(Source::Connect, &reason, Kwargs::new())
            }
            Err(HandlerError::Failed(error)) => {
                log::error!("{error}");
                // Could have accepted another command, ignore the result of this one
                let kwargs = kwargs_with(&[("error", error.into())], Kwargs::new());
                self.failed(Source::Wui, "Command error", Some(command_id), kwargs);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type Sent = Arc<Mutex<Vec<(consts::Event, Source, Option<i64>, Value)>>>;

    fn command() -> (Arc<Command>, Sent) {
        let sent: Sent = Arc::default();
        let log = sent.clone();
        let cb: EventCallback = Arc::new(move |event, source, _ts, command_id, kwargs| {
            log.lock()
                .unwrap()
                .push((event, source, command_id, Value::Object(kwargs)));
        });
        (Arc::new(Command::new(cb)), sent)
    }

    fn take(sent: &Sent) -> Vec<(consts::Event, Source, Option<i64>, Value)> {
        std::mem::take(&mut *sent.lock().unwrap())
    }

    use consts::Event::*;

    #[test]
    fn accept_run_finish() {
        let (cmd, sent) = command();
        cmd.set_handler(
            consts::Command::SendInfo,
            Arc::new(|c| {
                assert_eq!(c.kwargs(), Some(json!({"a": 1})));
                let mut finish = Finish::new(Source::Connect);
                finish.event = Some(Info);
                finish.kwargs.insert("sn".into(), "SN1".into());
                Ok(finish)
            }),
        );
        assert!(cmd.check_state(5, "SEND_INFO"));
        cmd.accept(5, "SEND_INFO", None, Some(json!({"a": 1})), false);
        assert!(cmd.new_cmd_evt.is_set() && !cmd.cmd_end_evt.is_set());
        cmd.run();
        assert_eq!(
            take(&sent),
            vec![
                (Accepted, Source::Connect, Some(5), json!({})),
                (Info, Source::Connect, Some(5), json!({"sn": "SN1"})),
            ]
        );
        assert_eq!((cmd.state(), cmd.last_state()), (None, Finished));
        assert!(cmd.cmd_end_evt.is_set() && !cmd.new_cmd_evt.is_set());
        // the same id again: resend its last state
        assert!(!cmd.check_state(5, "SEND_INFO"));
        assert_eq!(
            take(&sent),
            vec![(Finished, Source::Connect, Some(5), json!({}))]
        );
    }

    #[test]
    fn another_command_while_one_runs() {
        let (cmd, sent) = command();
        cmd.accept(1, "GCODE", None, None, true);
        take(&sent);
        assert!(!cmd.check_state(2, "GCODE"));
        assert!(!cmd.check_state(1, "GCODE"));
        assert_eq!(
            take(&sent),
            vec![
                (
                    Rejected,
                    Source::Connect,
                    Some(2),
                    json!({"reason": "Another command is running", "actual_command_id": 1})
                ),
                (Accepted, Source::Connect, Some(1), json!({})),
            ]
        );
        assert!(cmd.force());
    }

    #[test]
    fn unknown_and_not_implemented() {
        let (cmd, sent) = command();
        cmd.accept(3, "PRINT_LABEL", None, None, false);
        cmd.run();
        cmd.accept(4, "PAUSE_PRINT", None, None, false);
        cmd.run();
        let sent = take(&sent);
        assert_eq!(
            sent[1],
            (
                Rejected,
                Source::Wui,
                Some(3),
                json!({"reason": "Unknown command"})
            )
        );
        assert_eq!(
            sent[3],
            (
                Rejected,
                Source::Wui,
                Some(4),
                json!({"reason": "Not Implemented"})
            )
        );
        assert_eq!(cmd.last_state(), Rejected);
    }

    #[test]
    fn rejected_failed_and_panicking_handlers() {
        let (cmd, sent) = command();
        cmd.set_handler(
            consts::Command::StartPrint,
            Arc::new(|_| Err(HandlerError::Rejected("Printer not ready".into()))),
        );
        cmd.set_handler(
            consts::Command::DeleteFile,
            Arc::new(|_| Err(HandlerError::Failed("ValueError('x')".into()))),
        );
        cmd.set_handler(consts::Command::StopPrint, Arc::new(|_| panic!("boom")));
        for (id, name) in [(1, "START_PRINT"), (2, "DELETE_FILE"), (3, "STOP_PRINT")] {
            cmd.accept(id, name, None, None, false);
            cmd.run();
        }
        let sent: Vec<_> = take(&sent)
            .into_iter()
            .filter(|s| s.0 != Accepted)
            .collect();
        assert_eq!(
            sent,
            vec![
                (
                    Rejected,
                    Source::Connect,
                    Some(1),
                    json!({"reason": "Printer not ready"})
                ),
                (
                    Failed,
                    Source::Wui,
                    Some(2),
                    json!({"reason": "Command error", "error": "ValueError('x')"})
                ),
                (
                    Failed,
                    Source::Wui,
                    Some(3),
                    json!({"reason": "Command error", "error": "boom"})
                ),
            ]
        );
        assert_eq!(cmd.last_state(), Failed);
    }

    #[test]
    fn run_does_nothing_unless_accepted() {
        let (cmd, sent) = command();
        cmd.run();
        assert!(take(&sent).is_empty());
    }

    #[test]
    fn priority_command_stops_the_current_one() {
        let (cmd, sent) = command();
        cmd.accept(1, "GCODE", None, None, false);
        let stopper = Arc::downgrade(&cmd);
        cmd.set_stop_cb(Arc::new(move || {
            if let Some(cmd) = stopper.upgrade() {
                cmd.finish(Source::Wui, None, None, Kwargs::new());
            }
        }));
        assert!(cmd.check_state(2, "RESET_PRINTER"));
        assert_eq!(
            take(&sent).last(),
            Some(&(Finished, Source::Wui, Some(1), json!({})))
        );
    }
}

/// `tests/test_command.py` at `c0d7d233`, test by test.
#[cfg(test)]
mod sdk_tests {
    use super::*;
    use crate::models::Event;
    use serde_json::json;
    use std::sync::mpsc;

    struct Fixture {
        command: Arc<Command>,
        queue: mpsc::Receiver<Event>,
    }

    /// The `command` and `queue` fixtures: every event becomes a `models.Event` on the queue.
    fn fixture() -> Fixture {
        let (tx, queue) = mpsc::channel();
        let tx = Mutex::new(tx);
        let create_event: EventCallback =
            Arc::new(move |event, source, timestamp, command_id, kwargs| {
                let _ = tx
                    .lock()
                    .unwrap()
                    .send(Event::new(event, source, timestamp, command_id, kwargs));
            });
        Fixture {
            command: Arc::new(Command::new(create_event)),
            queue,
        }
    }

    /// Check right event is in queue.
    fn check_event(
        queue: &mpsc::Receiver<Event>,
        event: consts::Event,
        command_id: Option<i64>,
        timeout: Option<f64>,
    ) -> Event {
        let event_ = match timeout {
            None => queue.try_recv().expect("an event in the queue"),
            Some(timeout) => queue
                .recv_timeout(Duration::from_secs_f64(timeout))
                .expect("an event in time"),
        };
        assert_eq!(event_.event, event);
        if let Some(command_id) = command_id {
            assert_eq!(event_.command_id, Some(command_id));
        }
        event_
    }

    fn info_handler() -> CommandHandler {
        Arc::new(|caller| {
            assert_eq!(caller.args(), Some(json!([])));
            let mut finish = Finish::new(Source::Connect);
            finish.event = Some(consts::Event::Info);
            finish.kwargs.insert("x".into(), "x".into());
            Ok(finish)
        })
    }

    #[test]
    fn test_check_state() {
        let Fixture { command, queue } = fixture();
        // no conflict
        command.check_state(1, consts::Command::SendInfo.value());
        assert!(queue.try_recv().is_err());

        // last command_id is same
        command.set_command_id(1);
        command.set_last_state(consts::Event::Rejected);
        command.check_state(1, consts::Command::SendInfo.value());
        check_event(&queue, consts::Event::Rejected, None, None);

        // last command not finished yet
        command.set_state(Some(consts::Event::Accepted));
        command.check_state(1, consts::Command::SendInfo.value());
        check_event(&queue, consts::Event::Accepted, None, None);
    }

    #[test]
    fn test_accept() {
        let Fixture { command, queue } = fixture();
        command.accept(
            2,
            "TEST",
            Some(json!(["x"])),
            Some(json!({"param": "x"})),
            true,
        );
        assert_eq!(command.state(), Some(consts::Event::Accepted));
        assert_eq!(command.command_id(), 2);
        assert_eq!(command.args(), Some(json!(["x"])));
        assert_eq!(command.kwargs(), Some(json!({"param": "x"})));
        assert!(command.force());
        check_event(&queue, consts::Event::Accepted, None, None);
    }

    #[test]
    fn test_reject() {
        let Fixture { command, queue } = fixture();
        command.set_command_id(3);
        command.reject(Source::Wui, "No way", Kwargs::new());
        assert_eq!(command.last_state(), consts::Event::Rejected);
        assert_eq!(command.command_id(), 3);
        assert_eq!(command.state(), None);
        check_event(&queue, consts::Event::Rejected, None, None);
    }

    #[test]
    fn test_finish() {
        let Fixture { command, queue } = fixture();
        command.set_command_id(4);
        command.finish(
            Source::Marlin,
            Some(consts::Event::StateChanged),
            None,
            Kwargs::new(),
        );
        assert_eq!(command.last_state(), consts::Event::Finished);
        assert_eq!(command.state(), None);
        assert_eq!(command.command_id(), 4);
        check_event(&queue, consts::Event::StateChanged, None, None);
    }

    #[test]
    fn test_call() {
        let Fixture { command, queue } = fixture();
        command.set_command_name("SEND_INFO");
        command.set_state(Some(consts::Event::Accepted));
        command.set_handler(consts::Command::SendInfo, info_handler());
        command.run();
        let event = check_event(&queue, consts::Event::Info, None, None);
        assert_eq!(Value::Object(event.data), json!({"x": "x"}));
    }

    #[test]
    fn test_call_unknown_command() {
        let Fixture { command, queue } = fixture();
        command.set_command_name("TEST");
        command.set_state(Some(consts::Event::Accepted));
        command.run();
        let event = check_event(&queue, consts::Event::Rejected, None, None);
        assert_eq!(event.reason, Some(json!("Unknown command")));
    }

    #[test]
    fn test_call_not_implemented() {
        let Fixture { command, queue } = fixture();
        command.set_command_name("SEND_INFO");
        command.set_state(Some(consts::Event::Accepted));
        command.run();
        let event = check_event(&queue, consts::Event::Rejected, None, None);
        assert_eq!(event.reason, Some(json!("Not Implemented")));
    }

    #[test]
    fn test_call_exception() {
        let Fixture { command, queue } = fixture();
        command.set_handler(
            consts::Command::SendInfo,
            Arc::new(|caller| {
                let args = caller.args().map(|a| a.to_string()).unwrap_or_default();
                Err(HandlerError::Failed(format!("RuntimeError('{args}')")))
            }),
        );
        command.set_command_name("SEND_INFO");
        command.set_state(Some(consts::Event::Accepted));
        command.run();
        let event = check_event(&queue, consts::Event::Failed, None, None);
        assert_eq!(event.reason, Some(json!("Command error")));
        assert!(event.data.contains_key("error"));
    }

    #[test]
    fn test_unknown() {
        let Fixture { command, queue } = fixture();
        command.accept(24, "STANDUP", Some(json!([])), None, false);
        check_event(&queue, consts::Event::Accepted, None, None);

        command.run();
        check_event(&queue, consts::Event::Rejected, None, None);

        command.check_state(24, "STANDUP");
        check_event(&queue, consts::Event::Rejected, None, None);
    }

    #[test]
    fn test_command_recall() {
        let Fixture { command, queue } = fixture();
        command.set_handler(consts::Command::SendInfo, info_handler());
        command.accept(24, "SEND_INFO", Some(json!([])), None, false);
        check_event(&queue, consts::Event::Accepted, None, None);

        command.run();
        check_event(&queue, consts::Event::Info, None, None);

        command.check_state(24, consts::Command::SendInfo.value());
        check_event(&queue, consts::Event::Finished, None, None);
    }

    #[test]
    fn test_priority_command() {
        let Fixture { command, queue } = fixture();
        let event = Arc::new(Flag::new());

        let waiting = event.clone();
        command.set_handler(
            consts::Command::SendInfo,
            Arc::new(move |_| {
                waiting.wait(None);
                Err(HandlerError::Failed("RuntimeError('Failed!')".into()))
            }),
        );
        let setter = event.clone();
        command.set_stop_cb(Arc::new(move || {
            // threading.Timer(0.1, event.set)
            let setter = setter.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                setter.set();
            });
        }));
        command.accept(24, "SEND_INFO", Some(json!([])), None, false);
        check_event(&queue, consts::Event::Accepted, None, None);
        let runner = command.clone();
        let t = std::thread::spawn(move || runner.run());
        assert!(command.check_state(25, consts::Command::ResetPrinter.value()));
        event.wait(None);
        check_event(&queue, consts::Event::Failed, Some(24), Some(0.1));
        t.join().unwrap();
    }

    /// `TestCommandRejected`: a handler can reject a command with an exact reason
    #[test]
    fn test_reject_reason() {
        type Seen = Arc<Mutex<Vec<(consts::Event, Option<i64>, Kwargs)>>>;
        let events: Seen = Arc::default();
        let log = events.clone();
        let event_cb: EventCallback = Arc::new(move |event, _source, _ts, command_id, kwargs| {
            log.lock().unwrap().push((event, command_id, kwargs));
        });
        let command = Command::new(event_cb);
        command.set_handler(
            consts::Command::SendFileInfo,
            Arc::new(|_| Err(HandlerError::Rejected("File not found".into()))),
        );
        command.accept(
            42,
            consts::Command::SendFileInfo.value(),
            None,
            Some(json!({})),
            false,
        );
        command.run();
        let (event, command_id, kwargs) = events.lock().unwrap().last().cloned().unwrap();
        assert_eq!(event, consts::Event::Rejected);
        assert_eq!(kwargs["reason"], json!("File not found"));
        assert_eq!(command_id, Some(42));
        assert_eq!(command.state(), None);
    }
}
