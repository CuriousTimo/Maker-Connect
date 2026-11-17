//! Port of OctoPrint 1.11.8 `src/octoprint/plugins/action_command_prompt/__init__.py` (302 lines): the
//! wire parts. The plugin lets a printer ask a question through `//action:prompt_*` lines and answers
//! it with `M876 Sx` (the Prusa/Marlin "host prompt support").
//!
//! Original: Copyright (C) 2018 The OctoPrint Project, AGPL-3.0 (`__author__` Gina Häußge).
//!
//! # Ported (source line per item)
//!
//! `Prompt` (:13-31), `COMMAND = "M876"` (:41), `CAP_PROMPT_SUPPORT` (:42), `__init__` (:45),
//! `initialize` (:53), `on_event` (:86: `M876 P1` on connect, :92; the prompt closes on disconnect),
//! `get_settings_defaults` (:98), `action_command_handler` (:160), `gcode_queuing_handler` (:201,
//! force-sends `M876 Sx`, :224), `firmware_capability_handler` (:234, `M876 P1`, :240), `_show_prompt`
//! (:244), `_close_prompt` (:260), `_answer_prompt` (:269, answers with `force=True`, :277), and the
//! registration of three of the four `__plugin_hooks__` (:297-302): `octoprint.comm.protocol.action`,
//! `octoprint.comm.protocol.gcode.queuing` and `octoprint.comm.protocol.firmware.capabilities`, through
//! `plugin.rs` ([`ActionCommandPromptPlugin::register`]). The plugin is an `EventHandlerPlugin`, so its
//! `on_event` is registered with the event manager (`events.rs`).
//!
//! # Not ported (not wire)
//!
//! `get_additional_permissions` and the hook `octoprint.access.permissions` (:65, :301), `get_assets`
//! (:78), `on_settings_save` (:106), the API (`get_api_commands` :119, `on_api_command` :122,
//! `on_api_get` :136, `is_api_protected` :144) and `get_template_configs` (:149), and the plugin's
//! metadata (:285-296).
//!
//! # Ours
//!
//! 1. The settings (`enable`, `command`, `enable_emergency_sending`, `enable_signal_support`) are the
//!    defaults of `get_settings_defaults` (:98): our settings have no `config.yaml`, so
//!    `initialize` (:53) reads them from the defaults.
//! 2. Hooks are called with JSON arguments and without the comm instance (comm.rs, difference 20/31);
//!    `gcode_queuing_handler` reaches the comm through `self._printer`, the printer whose comm it is
//!    (`Printer::comm`), where Python uses its `comm_instance` argument. Its result `(None,)` is the JSON
//!    `[null]`; `_normalize_command_handler_result` drops the command for both.
//! 3. `self._printer.commands([...])` is `Printer::commands`; `self._plugin_manager.send_plugin_message`
//!    is `PluginManager::send_plugin_message` (a receiver is registered by the top edge).
//! 4. `self._command.format(choice=choice)` (:278): `choice` is passed to `str.format` as a `str`
//!    (`py_format.rs` takes `str`/`None` arguments only), where Python passes an `int`. A plain
//!    `{choice}` is the same text for both. A user-set `command` that has more than a plain
//!    `{choice}` as a field (a format spec `{choice:03d}`, a conversion `{choice!r}`, an attribute or
//!    an index) is `unimplemented!` (a known gap, `only_plain_choice`). The default `M876` has no
//!    `{choice}`.
//! 5. `prompt()` (the prompt's text and choices while it exists, the content of `on_api_get`, :148) and
//!    `prompt_is_active()` are OURS accessors for the top edge (M3's "Attention" state).

use super::comm::{py_split_none, py_strip};
use super::events::{Events, Listener, event_manager};
use super::plugin::{HookFn, PluginManager};
use super::py_attr::Attr;
use super::standard::Printer;
use serde_json::{Value, json};
use std::sync::Arc;

/// `plugin identifier` (the name of the plugin's package)
pub const IDENTIFIER: &str = "action_command_prompt";

/// `__init__.py:13 Prompt`
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub text: String,
    pub choices: Vec<String>,
    _active: bool,
}

impl Prompt {
    /// `:14 __init__(text)`
    pub fn new(text: &str) -> Prompt {
        Prompt {
            text: text.to_string(),
            choices: Vec::new(),
            _active: false,
        }
    }

    /// `:21 active` (a property)
    pub fn active(&self) -> bool {
        self._active
    }

    /// `:24 add_choice(text)`
    pub fn add_choice(&mut self, text: &str) {
        self.choices.push(text.to_string());
    }

    /// `:27 activate()`
    pub fn activate(&mut self) {
        self._active = true;
    }

    /// `:30 validate_choice(choice)`
    pub fn validate_choice(&self, choice: i64) -> bool {
        0 <= choice && (choice as usize) < self.choices.len()
    }
}

/// `__init__.py:34 ActionCommandPromptPlugin`
pub struct ActionCommandPromptPlugin {
    /// `self._printer` (injected by the plugin system in Python)
    _printer: Arc<Printer>,
    /// `self._prompt`
    _prompt: Attr<Option<Prompt>>,
    /// `self._enable`
    _enable: Attr<String>,
    /// `self._command`
    _command: Attr<String>,
    /// `self._enable_emergency_sending`
    _enable_emergency_sending: Attr<bool>,
    /// `self._enable_signal_support`
    _enable_signal_support: Attr<bool>,
    /// `self._cap_prompt_support`
    _cap_prompt_support: Attr<bool>,
}

impl ActionCommandPromptPlugin {
    pub const COMMAND: &'static str = "M876"; // :41
    pub const CAP_PROMPT_SUPPORT: &'static str = "PROMPT_SUPPORT"; // :42

    /// `:45 __init__()` followed by `:53 initialize()` (the settings are the defaults, ours 1)
    pub fn new(printer: Arc<Printer>) -> Arc<ActionCommandPromptPlugin> {
        let (enable, command, enable_emergency_sending, enable_signal_support) =
            Self::get_settings_defaults();
        Arc::new(ActionCommandPromptPlugin {
            _printer: printer,
            _prompt: Attr::new(None),
            _enable: Attr::new(enable),
            _command: Attr::new(command),
            _enable_emergency_sending: Attr::new(enable_emergency_sending),
            _enable_signal_support: Attr::new(enable_signal_support),
            _cap_prompt_support: Attr::new(false),
        })
    }

    /// `:98 get_settings_defaults()`: `enable`, `command`, `enable_emergency_sending`, `enable_signal_support`
    pub fn get_settings_defaults() -> (String, String, bool, bool) {
        (
            "detected".to_string(),
            Self::COMMAND.to_string(),
            true,
            true,
        )
    }

    /// `:297-302 __plugin_hooks__`, without `octoprint.access.permissions`, and the `EventHandlerPlugin`
    /// registration; into the global plugin manager and event manager. Call it before the
    /// `MachineCom` is created (it asks for its hooks in `__init__`).
    pub fn register(self: &Arc<Self>) {
        self.register_hooks(super::plugin::plugin_manager());
        let me = self.clone();
        let on_event: Listener = Arc::new(move |event, payload| me.on_event(event, payload));
        event_manager().register_event_handler_plugin(on_event);
    }

    /// the three wire hooks of `__plugin_hooks__` (:299-301) in `manager`
    pub fn register_hooks(self: &Arc<Self>, manager: &PluginManager) {
        let me = self.clone();
        let action: HookFn = Arc::new(move |args| {
            // action_command_handler(comm, line, action, *args, **kwargs): [line, action, name, params]
            me.action_command_handler(
                args[0].as_str().unwrap_or(""),
                args[1].as_str().unwrap_or(""),
            );
            Value::Null
        });
        manager.register_hook("octoprint.comm.protocol.action", IDENTIFIER, action);

        let me = self.clone();
        let queuing: HookFn = Arc::new(move |args| {
            // gcode_queuing_handler(comm_instance, phase, cmd, cmd_type, gcode, subcode=None, tags=None, ...)
            let text = |i: usize| args.get(i).and_then(Value::as_str);
            me.gcode_queuing_handler(text(0), text(1), text(2), text(3), text(4))
        });
        manager.register_hook("octoprint.comm.protocol.gcode.queuing", IDENTIFIER, queuing);

        let me = self.clone();
        let capabilities: HookFn = Arc::new(move |args| {
            // firmware_capability_handler(comm_instance, capability, enabled, already_defined, ...)
            me.firmware_capability_handler(
                args[0].as_str().unwrap_or(""),
                args.get(1).is_some_and(super::comm::py_truthy),
            );
            Value::Null
        });
        manager.register_hook(
            "octoprint.comm.protocol.firmware.capabilities",
            IDENTIFIER,
            capabilities,
        );
    }

    /// `:86 on_event(event, payload)`
    pub fn on_event(&self, event: &str, _payload: Option<&Value>) {
        if event == Events::CONNECTED
            && self._enable.get() == "always"
            && self._enable_signal_support.get()
        {
            self._printer
                .commands(&json!([format!("{} P1", self._command.get())]), None, false);
        } else if event == Events::DISCONNECTED {
            self._close_prompt();
        }
    }

    /// OURS (5): the prompt's text and choices while there is one (`on_api_get`, :136)
    pub fn prompt(&self) -> Option<(String, Vec<String>)> {
        self._prompt
            .with(|p| p.as_ref().map(|p| (p.text.clone(), p.choices.clone())))
    }

    /// OURS (5): `self._prompt is not None and self._prompt.active`
    pub fn prompt_is_active(&self) -> bool {
        self._prompt
            .with(|p| p.as_ref().is_some_and(Prompt::active))
    }

    /// `:122 on_api_command("select", data)`, `data["choice"] = choice`: what the API checks before it
    /// answers (:127-134); `Err((status, description))` is Flask's `abort`. OURS (the top edge calls it
    /// for an answer; `Permissions` and the JSON are not ported).
    pub fn api_select(&self, choice: i64) -> Result<(), (u16, String)> {
        if self._prompt.with(Option::is_none) {
            return Err((409, "No active prompt".to_string()));
        }
        if !self
            ._prompt
            .with(|p| p.as_ref().is_some_and(|p| p.validate_choice(choice)))
        {
            return Err((400, format!("{choice} is not a valid value for choice")));
        }
        self._answer_prompt(choice);
        Ok(())
    }

    // ~ action command handler

    /// `:160 action_command_handler(comm, line, action, *args, **kwargs)`
    pub fn action_command_handler(&self, _line: &str, action: &str) {
        if !action.starts_with("prompt_") {
            return;
        }

        let parts = py_split_none(action, 1);
        let (action, parameter) = if parts.len() == 1 {
            (parts[0], "")
        } else {
            (parts[0], parts[1])
        };

        if action == "prompt_begin" {
            if self
                ._prompt
                .with(|p| p.as_ref().is_some_and(Prompt::active))
            {
                // self._logger.warning("Prompt is already defined")
                return;
            }
            self._prompt.set(Some(Prompt::new(py_strip(parameter))));
        } else if action == "prompt_choice" || action == "prompt_button" {
            if self._prompt.with(|p| p.is_none()) {
                return;
            }
            if self
                ._prompt
                .with(|p| p.as_ref().is_some_and(Prompt::active))
            {
                // self._logger.warning("Prompt is already active")
                return;
            }
            self._prompt.with_mut(|p| {
                p.as_mut()
                    .expect("checked above")
                    .add_choice(py_strip(parameter))
            });
        } else if action == "prompt_show" {
            if self._prompt.with(|p| p.is_none()) {
                return;
            }
            if self
                ._prompt
                .with(|p| p.as_ref().is_some_and(Prompt::active))
            {
                // self._logger.warning("Prompt is already active")
                return;
            }
            self._show_prompt();
        } else if action == "prompt_end" {
            if self._prompt.with(|p| p.is_none()) {
                return;
            }
            self._close_prompt();
            self._prompt.set(None);
        }
    }

    // ~ queuing handling

    /// `:201 gcode_queuing_handler(comm_instance, phase, cmd, cmd_type, gcode, subcode=None, tags=None,
    /// *args, **kwargs)`: the hook's JSON result (`null`, or `[null]` for `(None,)`)
    pub fn gcode_queuing_handler(
        &self,
        _phase: Option<&str>,
        cmd: Option<&str>,
        _cmd_type: Option<&str>,
        gcode: Option<&str>,
        _subcode: Option<&str>,
    ) -> Value {
        let command = self._command.get();
        if gcode != Some(command.as_str()) {
            return Value::Null;
        }

        let enable = self._enable.get();
        if enable == "never"
            || (enable == "detected" && !self._cap_prompt_support.get())
            || !self._enable_emergency_sending.get()
        {
            return Value::Null;
        }

        let cmd = cmd.expect("TypeError: argument of type 'NoneType' is not iterable");
        if !cmd.contains('S') {
            // we only force-send M876 Sx
            return Value::Null;
        }

        // comm_instance._emergency_force_send(cmd, f"Force-sending {self._command} to the printer", gcode=gcode)
        let comm = self
            ._printer
            .comm()
            .expect("AttributeError: 'NoneType' object has no attribute '_emergency_force_send'");
        match comm._emergency_force_send(
            cmd,
            &format!("Force-sending {command} to the printer"),
            gcode,
        ) {
            None => Value::Null,
            Some(_) => json!([null]),
        }
    }

    // ~ capability reporting

    /// `:234 firmware_capability_handler(comm_instance, capability, enabled, already_defined, *args, **kwargs)`
    pub fn firmware_capability_handler(&self, capability: &str, enabled: bool) {
        if capability == Self::CAP_PROMPT_SUPPORT && enabled {
            self._cap_prompt_support.set(true);
            if self._enable.get() == "detected" && self._enable_signal_support.get() {
                self._printer.commands(
                    &json!([format!("{} P1", self._command.get())]),
                    None,
                    false,
                );
            }
        }
    }

    // ~ prompt handling

    /// `self._enable == "never" or (self._enable == "detected" and not self._cap_prompt_support)`, the
    /// guard of the three methods below (:245, :261, :270)
    fn _disabled(&self) -> bool {
        let enable = self._enable.get();
        enable == "never" || (enable == "detected" && !self._cap_prompt_support.get())
    }

    /// `:244 _show_prompt()`
    pub fn _show_prompt(&self) {
        if self._disabled() {
            return;
        }

        self._prompt.with_mut(|p| {
            p.as_mut()
                .expect("AttributeError: 'NoneType' object has no attribute 'activate'")
                .activate()
        });
        let (text, choices) = self.prompt().expect("the prompt exists");
        super::plugin::plugin_manager().send_plugin_message(
            IDENTIFIER,
            &json!({"action": "show", "text": text, "choices": choices}),
        );
    }

    /// `:260 _close_prompt()`
    pub fn _close_prompt(&self) {
        if self._disabled() {
            return;
        }

        self._prompt.set(None);
        super::plugin::plugin_manager()
            .send_plugin_message(IDENTIFIER, &json!({"action": "close"}));
    }

    /// `:269 _answer_prompt(choice)`
    pub fn _answer_prompt(&self, choice: i64) {
        if self._disabled() {
            return;
        }

        self._close_prompt();
        let command = self._command.get();
        if command.contains("{choice}") {
            only_plain_choice(&command);
            let formatted = super::py_format::format(
                &command,
                &[],
                &[("choice".to_string(), Some(choice.to_string()))],
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
            self._printer.commands(&json!([formatted]), None, true);
        } else {
            self._printer
                .commands(&json!([format!("{command} S{choice}")]), None, true);
        }
    }
}

/// OURS (4): `unimplemented!` when a replacement field of `command` names `choice` with more than a
/// plain `{choice}` (a spec, a conversion, an attribute or an index): `str.format` gives those an `int`
/// where `py_format` has a `str`. `{{` and `}}` are escapes (no field); nested fields of a format spec
/// are looked at too.
fn only_plain_choice(command: &str) {
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('{', Some('{')) | ('}', Some('}')) => i += 2,
            ('{', _) => {
                // the field up to its closing brace; nested braces are those of its format spec
                let mut depth = 1;
                let mut j = i + 1;
                while j < chars.len() && depth > 0 {
                    match chars[j] {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                let field: String = chars[i + 1..if depth == 0 { j - 1 } else { j }]
                    .iter()
                    .collect();
                if field.starts_with("choice") && field != "choice" {
                    unimplemented!(
                        "action_command_prompt: the command {command:?} formats `choice` ({{{field}}}) with more than a plain {{choice}}: `str.format` gets an int, ours a str"
                    );
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    //! All OURS: OctoPrint 1.11.8 has no test for `plugins/action_command_prompt`. Each test names the
    //! source line it checks. No test opens a serial port: the serial object is a recording fake, the
    //! hooks are registered in a local `PluginManager` handed to the comm (`_gcode_hooks` ...), never in
    //! the global one.
    use super::*;
    use crate::ports::octoprint::comm::{MachineCom, SerialError, SerialTransport};
    use crate::ports::octoprint::profile::PrinterProfileManager;
    use crate::ports::octoprint::py_collections::PyDict;
    use crate::ports::octoprint::standard::LocalFileManager;
    use std::sync::Mutex;

    /// a recording serial object
    struct Wire(Mutex<Vec<u8>>);

    impl Wire {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl SerialTransport for Wire {
        fn close(&self) -> Result<(), String> {
            Ok(())
        }
        fn str(&self) -> String {
            "Wire".to_string()
        }
        fn set_timeout(&self, _timeout: f64) {}
        fn timeout(&self) -> Option<f64> {
            None
        }
        fn set_baudrate(&self, _baudrate: i64) {}
        fn readline(&self) -> Result<Vec<u8>, SerialError> {
            Ok(Vec::new())
        }
        fn write(&self, data: &[u8]) -> Result<Option<i64>, SerialError> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(Some(data.len() as i64))
        }
    }

    /// a printer with an operational comm on a `Wire`, whose hooks are the plugin's
    fn setup() -> (
        Arc<ActionCommandPromptPlugin>,
        Arc<Printer>,
        Arc<MachineCom>,
        Arc<Wire>,
    ) {
        let profile = Arc::new(PrinterProfileManager::new());
        let printer = Printer::new(Arc::new(LocalFileManager), profile.clone());
        let plugin = ActionCommandPromptPlugin::new(printer.clone());
        let manager = PluginManager::new();
        plugin.register_hooks(&manager);

        let wire = Arc::new(Wire(Mutex::new(Vec::new())));
        let mut comm = MachineCom::new(None, None, None, Some(profile));
        {
            let c = Arc::get_mut(&mut comm).expect("nothing else holds the comm yet");
            c._printer_action_hooks = manager.get_hooks("octoprint.comm.protocol.action");
            let mut group = PyDict::new();
            group.insert(
                "info",
                manager.get_hooks("octoprint.comm.protocol.firmware.info"),
            );
            group.insert(
                "capabilities",
                manager.get_hooks("octoprint.comm.protocol.firmware.capabilities"),
            );
            group.insert(
                "capability_report",
                manager.get_hooks("octoprint.comm.protocol.firmware.capability_report"),
            );
            c._firmware_info_hooks = group;
            let mut gcode = PyDict::new();
            for phase in ["queuing", "queued", "sending", "sent"] {
                gcode.insert(
                    phase,
                    manager.get_hooks(&format!("octoprint.comm.protocol.gcode.{phase}")),
                );
            }
            c._gcode_hooks = gcode;
        }
        comm._serial.set(Some(wire.clone()));
        comm._state.set(MachineCom::STATE_OPERATIONAL);
        comm._firmware_info_received.set(true);
        printer.set_comm(Some(comm.clone()));
        (plugin, printer, comm, wire)
    }

    fn sent_lines(wire: &Wire, comm: &Arc<MachineCom>) -> Vec<String> {
        // what was queued for sending, then sent by hand (the send loop is not started): the queue entries
        let mut out = Vec::new();
        while let Ok(entry) = comm._send_queue.get(false, None) {
            comm._send_queue.task_done().unwrap();
            if let crate::ports::octoprint::comm::SendQueueEntry::Line {
                command: crate::ports::octoprint::comm::QueuedCommand::Command(s),
                ..
            } = entry
            {
                out.push(s);
            }
        }
        out.extend(wire.text().lines().map(str::to_string));
        out
    }

    /// OURS (4): a field with more than a plain `{choice}` is `unimplemented!`
    #[test]
    fn ours_command_with_more_than_a_plain_choice_is_unimplemented() {
        // the same text for a str and an int
        for ok in [
            "M876 S{choice}",
            "{choice}",
            "{{choice:03d}} {choice}",
            "{{{choice}}}",
            "G4 {choice} {choice}",
            "M876 S{choice} {{x}}",
        ] {
            only_plain_choice(ok);
        }
        for bad in [
            "M876 S{choice:03d} {choice}",
            "{choice} {choice!r}",
            "{choice} {choice.real}",
            "{choice} {choice[0]}",
            "{choice} {x:{choice:d}}",
        ] {
            let r = std::panic::catch_unwind(|| only_plain_choice(bad));
            let err = r.expect_err(bad);
            let text = err.downcast_ref::<String>().cloned().unwrap_or_default();
            assert!(
                text.starts_with("not implemented: action_command_prompt"),
                "{bad}: {text}"
            );
        }
    }

    #[test]
    fn ours_prompt_validate_choice() {
        // :30
        let mut p = Prompt::new("q");
        p.add_choice("a");
        p.add_choice("b");
        assert!(
            !p.validate_choice(-1)
                && p.validate_choice(0)
                && p.validate_choice(1)
                && !p.validate_choice(2)
        );
        assert!(!p.active());
        p.activate();
        assert!(p.active());
    }

    #[test]
    fn ours_defaults() {
        // :98
        assert_eq!(
            ("detected".to_string(), "M876".to_string(), true, true),
            ActionCommandPromptPlugin::get_settings_defaults()
        );
    }

    #[test]
    fn ours_nothing_is_shown_until_the_firmware_reports_prompt_support() {
        // :244-250 with `enable == "detected"`
        let (plugin, _printer, _comm, _wire) = setup();
        plugin.action_command_handler("//action:prompt_begin Hi", "prompt_begin Hi");
        plugin.action_command_handler("", "prompt_button Yes");
        plugin.action_command_handler("", "prompt_show");
        assert_eq!(
            Some(("Hi".to_string(), vec!["Yes".to_string()])),
            plugin.prompt()
        );
        assert!(!plugin.prompt_is_active());
    }

    #[test]
    fn ours_prompt_flow_after_prompt_support() {
        let (plugin, _printer, comm, wire) = setup();
        let messages: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let m2 = messages.clone();
        let receiver: crate::ports::octoprint::plugin::MessageReceiver =
            Arc::new(move |plugin, data| {
                if plugin == IDENTIFIER {
                    m2.lock().unwrap().push(data.clone());
                }
            });
        super::super::plugin::plugin_manager().register_message_receiver(receiver.clone());

        // :234-241: the capability makes the plugin send `M876 P1`
        plugin.firmware_capability_handler("PROMPT_SUPPORT", true);
        assert_eq!(vec!["M876 P1"], sent_lines(&wire, &comm));

        // :160-198: begin, choices (`prompt_choice` and `prompt_button`), show, a second begin is refused
        plugin.action_command_handler("", "prompt_begin  Change filament ");
        plugin.action_command_handler("", "prompt_choice Continue");
        plugin.action_command_handler("", "prompt_button   Cancel ");
        plugin.action_command_handler("", "prompt_show");
        assert!(plugin.prompt_is_active());
        plugin.action_command_handler("", "prompt_begin Other");
        plugin.action_command_handler("", "prompt_choice Late");
        assert_eq!(
            Some((
                "Change filament".to_string(),
                vec!["Continue".to_string(), "Cancel".to_string()]
            )),
            plugin.prompt()
        );
        // another action is not ours (:161)
        plugin.action_command_handler("", "print_start");

        // :269-282: an answer closes the prompt and is sent as `M876 S1` with force=True
        plugin._answer_prompt(1);
        assert!(plugin.prompt().is_none());
        assert_eq!(vec!["M876 S1"], sent_lines(&wire, &comm));

        // (the plugin manager is global: messages of other tests may be in the list, so look from our `show` on)
        let seen = messages.lock().unwrap().clone();
        let from_show: Vec<Value> = seen
            .into_iter()
            .skip_while(|m| m["action"] != "show")
            .take(2)
            .collect();
        assert_eq!(
            vec![
                json!({"action": "show", "text": "Change filament", "choices": ["Continue", "Cancel"]}),
                json!({"action": "close"}),
            ],
            from_show
        );
        super::super::plugin::plugin_manager().unregister_message_receiver(&receiver);

        // :189-198 prompt_end closes and forgets
        plugin.action_command_handler("", "prompt_begin Again");
        plugin.action_command_handler("", "prompt_end");
        assert!(plugin.prompt().is_none());
    }

    #[test]
    fn ours_m876_with_s_is_force_sent_through_the_hook() {
        // :201-231 through `MachineCom.sendCommand`: not forced before PROMPT_SUPPORT, with the
        // emergency parser and the prompt support M876 Sx is written at once (comm.py:5565)
        let (plugin, _printer, comm, wire) = setup();
        comm._capability_support
            .with_mut(|d| d.insert(MachineCom::CAPABILITY_EMERGENCY_PARSER, Some(true)));
        comm._firmware_capabilities
            .with_mut(|d| d.insert(MachineCom::CAPABILITY_EMERGENCY_PARSER, true));
        comm._clear_to_send.set();
        let cmd = |s: &str| crate::ports::octoprint::comm::QueuedCommand::Command(s.to_string());

        // detected, but no prompt support yet: the hook does nothing, the command is queued
        assert_eq!(
            Some(true),
            comm.send_command(cmd("M876 S1"), None, false, false, false, None, None)
        );
        assert_eq!("", wire.text());
        assert_eq!(vec!["M876 S1"], sent_lines(&wire, &comm));

        plugin._cap_prompt_support.set(true);
        // `M876 P1` has no S: queued
        assert_eq!(
            Some(true),
            comm.send_command(cmd("M876 P1"), None, false, false, false, None, None)
        );
        assert_eq!("", wire.text());
        assert_eq!(vec!["M876 P1"], sent_lines(&wire, &comm));
        // `M876 S1`: written ahead of the queue, not queued
        assert_eq!(
            Some(false),
            comm.send_command(cmd("M876 S1"), None, false, false, false, None, None)
        );
        assert_eq!("M876 S1\n", wire.text());
        // another gcode is none of its business (:213)
        assert_eq!(
            Some(true),
            comm.send_command(cmd("G1 X1 S1"), None, false, false, false, None, None)
        );
        assert_eq!(
            Value::Null,
            plugin.gcode_queuing_handler(Some("queuing"), Some("G1 S1"), None, Some("G1"), None)
        );

        // enable_emergency_sending false: no force
        plugin._enable_emergency_sending.set(false);
        assert_eq!(
            Value::Null,
            plugin.gcode_queuing_handler(
                Some("queuing"),
                Some("M876 S1"),
                None,
                Some("M876"),
                None
            )
        );
    }

    #[test]
    fn ours_enable_never_and_always() {
        let (plugin, printer, comm, wire) = setup();
        // "never": nothing is shown, closed or answered (:245, :261, :270)
        plugin._enable.set("never".to_string());
        plugin._prompt.set(Some(Prompt::new("q")));
        plugin._show_prompt();
        assert!(!plugin.prompt_is_active());
        plugin._close_prompt();
        assert!(plugin.prompt().is_some());
        plugin._answer_prompt(0);
        assert!(plugin.prompt().is_some());
        assert!(sent_lines(&wire, &comm).is_empty());
        // "always" asks for prompt support on connect (:86-92), and closes the prompt on disconnect
        plugin._enable.set("always".to_string());
        plugin.on_event(Events::CONNECTED, None);
        assert_eq!(vec!["M876 P1"], sent_lines(&wire, &comm));
        plugin.on_event(Events::DISCONNECTED, None);
        assert!(plugin.prompt().is_none());
        let _ = printer;
    }

    #[test]
    fn ours_registered_hooks() {
        // :297-301: three of the four hooks
        let (_plugin, printer, _comm, _wire) = setup();
        let plugin = ActionCommandPromptPlugin::new(printer);
        let manager = PluginManager::new();
        plugin.register_hooks(&manager);
        for hook in [
            "octoprint.comm.protocol.action",
            "octoprint.comm.protocol.gcode.queuing",
            "octoprint.comm.protocol.firmware.capabilities",
        ] {
            assert_eq!(
                vec!["action_command_prompt".to_string()],
                manager.get_hooks(hook).keys().cloned().collect::<Vec<_>>()
            );
        }
        assert!(manager.get_hooks("octoprint.access.permissions").is_empty());
    }
}
